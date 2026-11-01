//! Decoding recordings for the studio and writing edited exports: a port of
//! the decoding side of `RecordingStudioExporter.swift` (`pumpVideo`,
//! `pumpAudio`) and `RecordingCompositionBuilder.swift` on GStreamer.
//!
//! `FrameReader` pulls BGRx frames from a movie with seeking, for the preview
//! and the exporter alike. `AudioReader` mixes every audio track to 48 kHz
//! stereo PCM. `export` walks the edited timeline frame by frame, composites
//! each through `bettershot-render`'s studio compositor, and pushes video
//! and clip-cut, speed-adjusted audio into an H.264 encoder and muxer.

use crate::media;
use bettershot_core::recording::edit::{MaskSegment, StudioStyle, UnitRect};
use bettershot_core::recording::{ClipTimeline, PointerTimeline, StudioLayout, ViewportTimeline};
use bettershot_render::studio::{Frame, StudioCompositor};
use gst::prelude::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

const AUDIO_RATE: u64 = 48_000;
const AUDIO_CHANNELS: usize = 2;

// MARK: - Frames

pub struct FrameReader {
    pipeline: gst::Pipeline,
    sink: gst_app::AppSink,
    current: Option<Frame>,
    pending: Option<Frame>,
    generation: u64,
    eos: bool,
    pub width: u32,
    pub height: u32,
    pub duration: f64,
}

impl FrameReader {
    pub fn open(path: &Path) -> Result<Self, String> {
        media::init()?;
        let info = media::probe(path)?;
        if info.video_streams == 0 {
            return Err("The video has no picture track.".into());
        }
        let description = format!(
            "filesrc location=\"{}\" ! qtdemux name=demux demux.video_0 ! queue ! decodebin ! videoconvert ! video/x-raw,format=BGRx ! appsink name=sink sync=false max-buffers=4",
            media::escape(&path.to_string_lossy())
        );
        let pipeline = gst::parse::launch(&description)
            .map_err(|e| e.to_string())?
            .downcast::<gst::Pipeline>()
            .map_err(|_| "not a pipeline".to_string())?;
        let sink = pipeline.by_name("sink").and_then(|e| e.downcast::<gst_app::AppSink>().ok()).ok_or("no appsink")?;
        pipeline.set_state(gst::State::Paused).map_err(|e| e.to_string())?;
        let (result, _, _) = pipeline.state(gst::ClockTime::from_seconds(20));
        if result.is_err() {
            pipeline.set_state(gst::State::Null).ok();
            return Err("The video could not be decoded.".into());
        }
        pipeline.set_state(gst::State::Playing).map_err(|e| e.to_string())?;
        Ok(Self {
            pipeline,
            sink,
            current: None,
            pending: None,
            generation: 0,
            eos: false,
            width: info.width.max(0) as u32,
            height: info.height.max(0) as u32,
            duration: info.duration,
        })
    }

    pub fn current(&self) -> Option<&Frame> {
        self.current.as_ref()
    }

    /// Jumps to `seconds`; the next pull returns the frame at or before it.
    pub fn seek(&mut self, seconds: f64) -> Result<(), String> {
        let target = gst::ClockTime::from_nseconds((seconds.max(0.0) * 1e9) as u64);
        self.pipeline
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, target)
            .map_err(|e| e.to_string())?;
        self.pending = None;
        self.eos = false;
        Ok(())
    }

    fn pull(&mut self) -> Option<Frame> {
        if self.eos {
            return None;
        }
        let sample = match self.sink.try_pull_sample(gst::ClockTime::from_seconds(20)) {
            Some(sample) => sample,
            None => {
                self.eos = true;
                return None;
            }
        };
        let caps = sample.caps()?;
        let structure = caps.structure(0)?;
        let width = structure.get::<i32>("width").ok()? as u32;
        let height = structure.get::<i32>("height").ok()? as u32;
        let buffer = sample.buffer()?;
        let pts = buffer.pts().map(|t| t.seconds_f64()).unwrap_or(0.0);
        let map = buffer.map_readable().ok()?;
        let data = map.as_slice().to_vec();
        let stride = if data.len() == (width * height * 4) as usize {
            width as usize * 4
        } else {
            data.len() / height.max(1) as usize
        };
        self.generation += 1;
        Some(Frame { width, height, stride, data, pts, generation: self.generation })
    }

    /// The frame showing at `seconds`, pulling forward from the current
    /// position or seeking when the time is behind it or far ahead.
    pub fn frame_at(&mut self, seconds: f64) -> Option<&Frame> {
        // Past the end, the last frame holds.
        let seconds = if self.duration > 0.0 { seconds.min(self.duration - 0.001).max(0.0) } else { seconds.max(0.0) };
        let needs_seek = match &self.current {
            Some(current) => seconds < current.pts - 0.001 || seconds - current.pts > 2.0,
            None => self.eos,
        };
        let mut previous = None;
        if needs_seek {
            self.seek(seconds).ok()?;
            previous = self.current.take();
        }
        while let Some(candidate) = self.pending.take().or_else(|| self.pull()) {
            if candidate.pts <= seconds + 0.0005 || self.current.is_none() {
                self.current = Some(candidate);
            } else {
                self.pending = Some(candidate);
                break;
            }
        }
        if self.current.is_none() {
            self.current = previous;
        }
        self.current.as_ref()
    }
}

impl Drop for FrameReader {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

// MARK: - Audio

pub struct AudioReader {
    pipeline: gst::Pipeline,
    sink: gst_app::AppSink,
    eos: bool,
}

/// A chunk of interleaved stereo samples starting at `pts` seconds.
pub struct AudioChunk {
    pub pts: f64,
    pub samples: Vec<i16>,
}

impl AudioChunk {
    pub fn frames(&self) -> usize {
        self.samples.len() / AUDIO_CHANNELS
    }
}

impl AudioReader {
    /// None when the movie has no audio.
    pub fn open(path: &Path) -> Result<Option<Self>, String> {
        media::init()?;
        let info = media::probe(path)?;
        if info.audio_streams == 0 {
            return Ok(None);
        }
        let mut description = format!(
            "filesrc location=\"{}\" ! qtdemux name=demux audiomixer name=mix ! audioconvert ! audioresample ! audio/x-raw,format=S16LE,rate={AUDIO_RATE},channels={AUDIO_CHANNELS},layout=interleaved ! appsink name=sink sync=false max-buffers=16",
            media::escape(&path.to_string_lossy())
        );
        for index in 0..info.audio_streams {
            description.push_str(&format!(
                " demux.audio_{index} ! queue ! decodebin ! audioconvert ! audioresample ! audio/x-raw,format=S16LE,rate={AUDIO_RATE},channels={AUDIO_CHANNELS} ! mix."
            ));
        }
        let pipeline = gst::parse::launch(&description)
            .map_err(|e| e.to_string())?
            .downcast::<gst::Pipeline>()
            .map_err(|_| "not a pipeline".to_string())?;
        let sink = pipeline.by_name("sink").and_then(|e| e.downcast::<gst_app::AppSink>().ok()).ok_or("no appsink")?;
        pipeline.set_state(gst::State::Paused).map_err(|e| e.to_string())?;
        let (result, _, _) = pipeline.state(gst::ClockTime::from_seconds(20));
        if result.is_err() {
            pipeline.set_state(gst::State::Null).ok();
            return Err("The audio could not be decoded.".into());
        }
        pipeline.set_state(gst::State::Playing).map_err(|e| e.to_string())?;
        Ok(Some(Self { pipeline, sink, eos: false }))
    }

    pub fn seek(&mut self, seconds: f64) -> Result<(), String> {
        let target = gst::ClockTime::from_nseconds((seconds.max(0.0) * 1e9) as u64);
        self.pipeline
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, target)
            .map_err(|e| e.to_string())?;
        self.eos = false;
        Ok(())
    }

    pub fn next_chunk(&mut self) -> Option<AudioChunk> {
        if self.eos {
            return None;
        }
        let Some(sample) = self.sink.try_pull_sample(gst::ClockTime::from_seconds(20)) else {
            self.eos = true;
            return None;
        };
        let buffer = sample.buffer()?;
        let pts = buffer.pts().map(|t| t.seconds_f64()).unwrap_or(0.0);
        let map = buffer.map_readable().ok()?;
        let bytes = map.as_slice();
        let samples = bytes.chunks_exact(2).map(|pair| i16::from_le_bytes([pair[0], pair[1]])).collect();
        Some(AudioChunk { pts, samples })
    }
}

impl Drop for AudioReader {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

// MARK: - Export

#[derive(Debug)]
pub enum ExportError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "The export was cancelled."),
            Self::Failed(detail) => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<String> for ExportError {
    fn from(detail: String) -> Self {
        Self::Failed(detail)
    }
}

/// Everything an export needs; the app resolves it from the project.
pub struct ExportRequest {
    pub screen: PathBuf,
    pub camera: Option<PathBuf>,
    /// Seconds between the first screen frame and the first camera frame.
    pub camera_lead_in: f64,
    pub style: StudioStyle,
    pub layout: StudioLayout,
    pub clips: ClipTimeline,
    pub viewport: ViewportTimeline,
    pub crop: UnitRect,
    pub masks: Vec<MaskSegment>,
    /// The synthetic pointer, when the capture hid the system cursor.
    pub pointer: Option<PointerTimeline>,
    pub shows_press_effects: bool,
    pub canvas: (u32, u32),
    pub frames_per_second: u32,
    /// x264 constant quality (lower is better).
    pub crf: u32,
    pub preset: String,
    pub include_audio: bool,
    pub audio_bitrate: u32,
    pub output: PathBuf,
}

struct Resampler {
    speed: f64,
    position: f64,
}

impl Resampler {
    /// Nearest-neighbour retiming: `speed` input frames per output frame.
    fn process(&mut self, input: &[i16], output: &mut Vec<i16>) {
        let frames = input.len() / AUDIO_CHANNELS;
        while (self.position as usize) < frames {
            let index = self.position as usize;
            output.extend_from_slice(&input[index * AUDIO_CHANNELS..index * AUDIO_CHANNELS + AUDIO_CHANNELS]);
            self.position += self.speed;
        }
        self.position -= frames as f64;
    }
}

/// Feeds the clip-cut soundtrack in step with the video frames.
struct AudioFeeder {
    reader: AudioReader,
    source: gst_app::AppSrc,
    clips: Vec<bettershot_core::recording::ClipSegment>,
    clip_index: usize,
    /// Source seconds consumed in the current clip; None before its seek.
    consumed: Option<f64>,
    resampler: Resampler,
    pushed_frames: u64,
    finished: bool,
}

impl AudioFeeder {
    fn pushed_seconds(&self) -> f64 {
        self.pushed_frames as f64 / AUDIO_RATE as f64
    }

    fn push(&mut self, samples: Vec<i16>) -> Result<(), ExportError> {
        if samples.is_empty() {
            return Ok(());
        }
        let frames = (samples.len() / AUDIO_CHANNELS) as u64;
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let mut buffer = gst::Buffer::from_mut_slice(bytes);
        {
            let buffer = buffer.get_mut().ok_or_else(|| ExportError::Failed("buffer".into()))?;
            buffer.set_pts(gst::ClockTime::from_nseconds(self.pushed_frames * 1_000_000_000 / AUDIO_RATE));
            buffer.set_duration(gst::ClockTime::from_nseconds(frames * 1_000_000_000 / AUDIO_RATE));
        }
        self.pushed_frames += frames;
        self.source.push_buffer(buffer).map_err(|e| ExportError::Failed(format!("audio push failed: {e:?}")))?;
        Ok(())
    }

    /// Pushes audio until the soundtrack reaches `editor_time`.
    fn fill_until(&mut self, editor_time: f64) -> Result<(), ExportError> {
        while !self.finished && self.pushed_seconds() < editor_time {
            let Some(clip) = self.clips.get(self.clip_index).cloned() else {
                self.finished = true;
                break;
            };
            if self.consumed.is_none() {
                self.reader.seek(clip.source_start)?;
                self.consumed = Some(clip.source_start);
                self.resampler = Resampler { speed: clip.speed, position: 0.0 };
            }
            let consumed = self.consumed.unwrap_or(clip.source_start);
            let Some(chunk) = self.reader.next_chunk() else {
                // The recording's audio ends before the clip does: silence fills it.
                let remaining = ((clip.source_end - consumed).max(0.0) / clip.speed * AUDIO_RATE as f64) as usize;
                self.push(vec![0; remaining * AUDIO_CHANNELS])?;
                self.clip_index += 1;
                self.consumed = None;
                continue;
            };
            let chunk_end = chunk.pts + chunk.frames() as f64 / AUDIO_RATE as f64;
            if chunk_end <= consumed {
                continue;
            }
            let start_frame = (((consumed - chunk.pts).max(0.0)) * AUDIO_RATE as f64).round() as usize;
            let end_frame =
                (((clip.source_end - chunk.pts).min(chunk_end - chunk.pts)) * AUDIO_RATE as f64).round() as usize;
            let end_frame = end_frame.min(chunk.frames());
            if end_frame > start_frame {
                let input = &chunk.samples[start_frame * AUDIO_CHANNELS..end_frame * AUDIO_CHANNELS];
                let mut output = Vec::with_capacity(input.len());
                self.resampler.process(input, &mut output);
                self.push(output)?;
            }
            self.consumed = Some(chunk_end.min(clip.source_end));
            if chunk_end >= clip.source_end {
                self.clip_index += 1;
                self.consumed = None;
            }
        }
        Ok(())
    }
}

/// Writes the edited movie. `progress` receives 0...1; `cancelled` is polled
/// between frames, and a cancelled export removes its partial file.
pub fn export(
    request: &ExportRequest,
    progress: &dyn Fn(f64),
    cancelled: &dyn Fn() -> bool,
) -> Result<(), ExportError> {
    media::init()?;
    let (width, height) = (request.canvas.0.max(2) & !1, request.canvas.1.max(2) & !1);
    let fps = request.frames_per_second.clamp(1, 240);
    let container = request.output.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_ascii_lowercase();
    let muxer = match container.as_str() {
        "mov" => "qtmux",
        "mp4" => "mp4mux",
        other => return Err(ExportError::Failed(format!("Videos cannot be exported as .{other}."))),
    };
    let encoder = crate::recorder::video_encoder_named(fps as i64, request.crf, &request.preset)
        .ok_or_else(|| ExportError::Failed("No H.264 encoder is installed.".into()))?;
    if let Some(parent) = request.output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ExportError::Failed(e.to_string()))?;
    }
    let mut description = format!(
        "appsrc name=video format=time block=true max-bytes=67108864 caps=video/x-raw,format=BGRx,width={width},height={height},framerate={fps}/1 ! queue max-size-buffers=8 max-size-bytes=0 max-size-time=0 ! videoconvert ! video/x-raw,format=I420 ! {encoder} ! h264parse ! queue ! mux. {muxer} name=mux ! filesink location=\"{}\"",
        media::escape(&request.output.to_string_lossy())
    );
    let audio_reader = if request.include_audio { AudioReader::open(&request.screen)? } else { None };
    let audio_encoder = crate::recorder::audio_encoder();
    let has_audio = audio_reader.is_some() && audio_encoder.is_some();
    if has_audio {
        let aac = audio_encoder.unwrap_or("voaacenc");
        description.push_str(&format!(
            " appsrc name=audio format=time block=true max-bytes=16777216 caps=audio/x-raw,format=S16LE,rate={AUDIO_RATE},channels={AUDIO_CHANNELS},layout=interleaved ! queue max-size-buffers=64 max-size-bytes=0 max-size-time=0 ! audioconvert ! {aac} bitrate={} ! queue ! mux.",
            request.audio_bitrate
        ));
    }
    let pipeline = gst::parse::launch(&description)
        .map_err(|e| ExportError::Failed(e.to_string()))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| ExportError::Failed("not a pipeline".into()))?;
    let video_source = pipeline
        .by_name("video")
        .and_then(|e| e.downcast::<gst_app::AppSrc>().ok())
        .ok_or_else(|| ExportError::Failed("no video source".into()))?;
    let bus = pipeline.bus().ok_or_else(|| ExportError::Failed("no bus".into()))?;
    pipeline.set_state(gst::State::Playing).map_err(|e| ExportError::Failed(e.to_string()))?;

    let result = (|| -> Result<(), ExportError> {
        let mut screen = FrameReader::open(&request.screen)?;
        let mut camera = match &request.camera {
            Some(path) if request.layout.bubble_rect.width > 0.0 => Some(FrameReader::open(path)?),
            _ => None,
        };
        let mut audio = match audio_reader.filter(|_| has_audio) {
            Some(reader) => Some(AudioFeeder {
                reader,
                source: pipeline
                    .by_name("audio")
                    .and_then(|e| e.downcast::<gst_app::AppSrc>().ok())
                    .ok_or_else(|| ExportError::Failed("no audio source".into()))?,
                clips: request.clips.segments.clone(),
                clip_index: 0,
                consumed: None,
                resampler: Resampler { speed: 1.0, position: 0.0 },
                pushed_frames: 0,
                finished: false,
            }),
            None => None,
        };
        let mut compositor = StudioCompositor::new(
            (f64::from(width), f64::from(height)),
            request.style.clone(),
            request.layout.clone(),
            request.viewport.clone(),
            request.crop,
            request.masks.clone(),
        )
        .with_pointer(request.pointer.clone(), request.shows_press_effects);
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width as i32, height as i32)
            .map_err(|e| ExportError::Failed(e.to_string()))?;
        let duration = request.clips.duration();
        let total = ((duration * f64::from(fps)).round() as u64).max(1);
        let frame_duration = gst::ClockTime::from_nseconds(1_000_000_000 / u64::from(fps));
        let mut last_segment: Option<usize> = None;
        let fallback = Frame::solid(screen.width.max(2), screen.height.max(2), (0, 0, 0), 0.0, 0);
        for index in 0..total {
            if cancelled() {
                return Err(ExportError::Cancelled);
            }
            let editor_time = index as f64 / f64::from(fps);
            let location = request.clips.location(editor_time);
            let source_time = location.as_ref().map(|l| l.source_time).unwrap_or(0.0);
            let segment = location.as_ref().map(|l| l.segment_index);
            if segment != last_segment {
                screen.seek(source_time)?;
                if let Some(camera) = camera.as_mut() {
                    camera.seek((source_time - request.camera_lead_in).max(0.0))?;
                }
                last_segment = segment;
            }
            let screen_frame = screen.frame_at(source_time).cloned();
            let camera_frame = match camera.as_mut() {
                Some(camera) if source_time - request.camera_lead_in >= 0.0 => {
                    camera.frame_at(source_time - request.camera_lead_in).cloned()
                }
                _ => None,
            };
            compositor.render_into(
                &surface,
                screen_frame.as_ref().unwrap_or(&fallback),
                camera_frame.as_ref(),
                editor_time,
            );
            let bytes = {
                surface.flush();
                let stride = surface.stride() as usize;
                let data = surface.data().map_err(|e| ExportError::Failed(e.to_string()))?;
                let row = width as usize * 4;
                if stride == row {
                    data.to_vec()
                } else {
                    let mut packed = Vec::with_capacity(row * height as usize);
                    for y in 0..height as usize {
                        packed.extend_from_slice(&data[y * stride..y * stride + row]);
                    }
                    packed
                }
            };
            let mut buffer = gst::Buffer::from_mut_slice(bytes);
            {
                let buffer = buffer.get_mut().ok_or_else(|| ExportError::Failed("buffer".into()))?;
                buffer.set_pts(gst::ClockTime::from_nseconds(index * 1_000_000_000 / u64::from(fps)));
                buffer.set_duration(frame_duration);
            }
            video_source.push_buffer(buffer).map_err(|e| ExportError::Failed(format!("video push failed: {e:?}")))?;
            if let Some(audio) = audio.as_mut() {
                audio.fill_until(editor_time + 1.0 / f64::from(fps))?;
            }
            if let Ok(gst::MessageView::Error(error)) = bus.pop().as_ref().map(|m| m.view()).ok_or(()) {
                return Err(ExportError::Failed(format!("{} ({})", error.error(), error.debug().unwrap_or_default())));
            }
            progress(index as f64 / total as f64);
        }
        if let Some(audio) = audio.as_mut() {
            audio.fill_until(duration)?;
            let _ = audio.source.end_of_stream();
        }
        let _ = video_source.end_of_stream();
        let started = std::time::Instant::now();
        loop {
            match bus.timed_pop(gst::ClockTime::from_mseconds(200)) {
                Some(message) => match message.view() {
                    gst::MessageView::Eos(_) => break,
                    gst::MessageView::Error(error) => {
                        return Err(ExportError::Failed(format!(
                            "{} ({})",
                            error.error(),
                            error.debug().unwrap_or_default()
                        )));
                    }
                    _ => {}
                },
                None if started.elapsed() > Duration::from_secs(120) => {
                    return Err(ExportError::Failed("The encoder did not finish.".into()));
                }
                None => {}
            }
        }
        Ok(())
    })();
    let _ = pipeline.set_state(gst::State::Null);
    if result.is_err() {
        let _ = std::fs::remove_file(&request.output);
    } else {
        progress(1.0);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::{Recorder, RecordingRequest};
    use bettershot_core::annotation::background::{BgColor, BgStyle};
    use bettershot_core::recording::{ClipSegment, ContentMode, SourceMode};

    fn fixture_recording(dir: &Path) -> Option<PathBuf> {
        if std::env::var_os("BETTERSHOT_FAKE_SCREEN").is_none() || media::init().is_err() {
            eprintln!("skipped: set BETTERSHOT_TESTING=1 and BETTERSHOT_FAKE_SCREEN to a PNG");
            return None;
        }
        let output = dir.join("screen.mov");
        let request = RecordingRequest {
            mode: SourceMode::Fullscreen,
            area: None,
            display: None,
            screen_output: output.clone(),
            camera_output: None,
            frame_rate: 30,
            system_audio: true,
            microphone: None,
            camera: None,
            restore_token: None,
        };
        let finished = gtk::glib::MainContext::default().block_on(async {
            let (recorder, _) = Recorder::start(None, request).await.expect("recording starts");
            gtk::glib::timeout_future(Duration::from_millis(2600)).await;
            recorder.stop().await
        });
        assert!(finished.file_is_usable, "{finished:?}");
        Some(output)
    }

    #[test]
    fn reads_frames_with_seeking_and_exports_an_edited_movie() {
        let dir = tempfile::tempdir().unwrap();
        let Some(screen) = fixture_recording(dir.path()) else { return };
        let mut reader = FrameReader::open(&screen).unwrap();
        assert_eq!((reader.width, reader.height), (1280, 800));
        let first = reader.frame_at(0.0).unwrap().clone();
        assert!(first.pts < 0.05);
        assert_eq!(first.data.len(), first.stride * 800);
        let later = reader.frame_at(1.5).unwrap().pts;
        assert!(later > 1.3 && later <= 1.5, "{later}");
        let back = reader.frame_at(0.5).unwrap().pts;
        assert!(back > 0.3 && back <= 0.5, "seeking backwards: {back}");
        assert!(reader.frame_at(99.0).is_some(), "past the end holds the last frame");
        let mut audio = AudioReader::open(&screen).unwrap().expect("the recording has audio");
        audio.seek(1.0).unwrap();
        let chunk = audio.next_chunk().unwrap();
        assert!(chunk.pts <= 1.05 && chunk.frames() > 0);

        // Cut the middle out, slow the first half, and frame it on a color.
        let info = media::probe(&screen).unwrap();
        let clips = ClipTimeline::new(vec![
            ClipSegment::with_speed(0.0, 1.0, 0.5),
            ClipSegment::with_speed(info.duration - 1.0, info.duration, 1.0),
        ])
        .normalized(info.duration);
        let style = StudioStyle {
            background: BgStyle::Solid(BgColor::new("b", "Blue", 0.0, 0.0, 1.0)),
            padding: 0.1,
            ..Default::default()
        };
        let canvas = (640u32, 400u32);
        let layout = StudioLayout::make((640.0, 400.0), &style, false, None, ContentMode::Fill);
        let output = dir.path().join("Edited.mp4");
        let request = ExportRequest {
            screen: screen.clone(),
            camera: None,
            camera_lead_in: 0.0,
            style,
            layout,
            clips: clips.clone(),
            viewport: ViewportTimeline::identity(),
            crop: UnitRect::UNIT,
            masks: Vec::new(),
            pointer: None,
            shows_press_effects: true,
            canvas,
            frames_per_second: 30,
            crf: 26,
            preset: "veryfast".into(),
            include_audio: true,
            audio_bitrate: 128_000,
            output: output.clone(),
        };
        let reported = std::cell::RefCell::new(Vec::new());
        export(&request, &|p| reported.borrow_mut().push(p), &|| false).unwrap();
        let exported = media::probe(&output).unwrap();
        assert_eq!((exported.width, exported.height), (640, 400));
        assert_eq!(exported.audio_streams, 1, "the two tracks are mixed into one");
        assert!((exported.duration - clips.duration()).abs() < 0.25, "{} vs {}", exported.duration, clips.duration());
        assert_eq!(reported.borrow().last().copied(), Some(1.0));
        let mut check = FrameReader::open(&output).unwrap();
        let frame = check.frame_at(0.5).unwrap();
        let corner = &frame.data[4 * 4..4 * 4 + 3];
        assert!(corner[0] > 220 && corner[1] < 40 && corner[2] < 40, "the padding is blue (BGRx): {corner:?}");
        let center = &frame.data[200 * frame.stride + 320 * 4..200 * frame.stride + 320 * 4 + 3];
        assert!(center[0] < 200 || center[1] > 40, "the card shows the recording: {center:?}");

        // Cancelling removes the partial file.
        let cancelled = dir.path().join("Cancelled.mp4");
        let request = ExportRequest { output: cancelled.clone(), ..request };
        let result = export(&request, &|_| {}, &|| true);
        assert!(matches!(result, Err(ExportError::Cancelled)));
        assert!(!cancelled.exists());
    }
}
