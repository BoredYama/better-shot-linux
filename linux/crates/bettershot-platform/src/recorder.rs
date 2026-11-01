//! Screen recording: the ScreenCast portal hands over a PipeWire stream of a
//! display or window (the portal's own picker chooses it), and a GStreamer
//! pipeline encodes it into the package's `screen.mov`, with system audio and
//! a microphone as separate tracks and a camera as `camera.mov`. The movie is
//! written fragmented, so a crash leaves a playable file up to the last
//! fragment, as the macOS writer does.
//!
//! The cursor is embedded in the pixels: GStreamer's `pipewiresrc` drops the
//! stream's cursor metadata, so a separate pointer timeline (`input.json`)
//! needs the raw PipeWire API and waits for the video editor's compositor.
//!
//! Under `BETTERSHOT_TESTING=1` with `BETTERSHOT_FAKE_SCREEN`, the fixture
//! image plays as a live source instead of the portal, so recordings can be
//! checked without a desktop; audio and camera use test sources then.

use crate::media;
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType, Stream};
use ashpd::desktop::{PersistMode, ResponseError, Session};
use ashpd::WindowIdentifier;
use bettershot_core::geometry::Rect;
use bettershot_core::recording::SourceMode;
use gst::prelude::*;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct RecordingRequest {
    pub mode: SourceMode,
    /// For `SourceMode::Area`: the area in the desktop's logical layout.
    pub area: Option<Rect>,
    pub screen_output: PathBuf,
    pub camera_output: Option<PathBuf>,
    pub frame_rate: i64,
    pub system_audio: bool,
    /// A PulseAudio source name; None records no narration.
    pub microphone: Option<String>,
    /// A V4L2 device path; None records no camera.
    pub camera: Option<String>,
    /// The portal's token from the previous recording, which skips its picker.
    pub restore_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CameraResult {
    /// Seconds between the first screen frame and the first camera frame.
    pub lead_in: f64,
    pub width: i64,
    pub height: i64,
}

/// What a finished recording left on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    pub duration: f64,
    pub width: i64,
    pub height: i64,
    pub pixel_scale: f64,
    pub includes_system_audio: bool,
    pub includes_microphone: bool,
    pub camera: Option<CameraResult>,
    pub restore_token: Option<String>,
    /// The error that ended the recording early, if any. The file may still
    /// be usable up to that point.
    pub error: Option<String>,
    pub file_is_usable: bool,
}

#[derive(Debug)]
pub enum StartError {
    /// The user dismissed the portal's picker.
    Cancelled,
    /// No way to read the screen stream exists on this desktop.
    Unavailable(String),
    Failed(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "The recording was cancelled."),
            Self::Unavailable(detail) => write!(
                f,
                "Screen recording needs the ScreenCast portal and PipeWire. Install xdg-desktop-portal with the backend for your desktop (GNOME, KDE, or wlr) and GStreamer's PipeWire plugin. {detail}"
            ),
            Self::Failed(detail) => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for StartError {}

/// Inputs that could not be used; the recording continues without them.
pub type Warnings = Vec<String>;

enum Source {
    Portal { session: Session<Screencast>, stream: Stream, fd: OwnedFd, restore_token: Option<String> },
    Fixture(PathBuf),
}

#[derive(Default)]
struct Timing {
    first_screen_pts: Option<gst::ClockTime>,
    first_camera_pts: Option<gst::ClockTime>,
}

pub struct Recorder {
    pipeline: gst::Pipeline,
    source: Source,
    request: RecordingRequest,
    includes_system_audio: bool,
    includes_microphone: bool,
    includes_camera: bool,
    pixel_scale: f64,
    timing: Arc<Mutex<Timing>>,
    bus_events: async_channel::Receiver<BusEvent>,
    _watch: gst::bus::BusWatchGuard,
    error: Option<String>,
}

enum BusEvent {
    Eos,
    Error(String),
}

fn uses_fixture() -> Option<PathBuf> {
    if std::env::var("BETTERSHOT_TESTING").as_deref() != Ok("1") {
        return None;
    }
    let fixture = PathBuf::from(std::env::var_os("BETTERSHOT_FAKE_SCREEN")?);
    if fixture.is_dir() {
        // A folder of frames: the first one stands in for the screen.
        let mut frames: Vec<PathBuf> = std::fs::read_dir(&fixture)
            .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        frames.sort();
        frames.into_iter().next()
    } else {
        Some(fixture)
    }
}

fn video_encoder(frame_rate: i64) -> Option<String> {
    let keyframes = frame_rate.clamp(1, 240) * 2;
    let candidates = [
        ("x264enc", format!("speed-preset=veryfast tune=zerolatency pass=qual quantizer=21 key-int-max={keyframes}")),
        ("vah264enc", String::new()),
        ("vaapih264enc", String::new()),
        ("openh264enc", format!("gop-size={keyframes} rate-control=quality")),
    ];
    candidates
        .into_iter()
        .find(|(name, _)| gst::ElementFactory::find(name).is_some())
        .map(|(name, props)| format!("{name} {props}"))
}

fn audio_encoder() -> Option<&'static str> {
    ["fdkaacenc", "voaacenc", "avenc_aac", "opusenc"].into_iter().find(|name| gst::ElementFactory::find(name).is_some())
}

fn classify(error: ashpd::Error) -> StartError {
    match error {
        ashpd::Error::Response(ResponseError::Cancelled) => StartError::Cancelled,
        ashpd::Error::PortalNotFound(_) | ashpd::Error::Zbus(_) => StartError::Unavailable(error.to_string()),
        ashpd::Error::Portal(ref portal) if portal.to_string().contains("NotFound") => {
            StartError::Unavailable(error.to_string())
        }
        other => StartError::Failed(format!("The screen cast portal failed: {other}")),
    }
}

async fn portal_source(parent: Option<&WindowIdentifier>, request: &RecordingRequest) -> Result<Source, StartError> {
    let proxy = Screencast::new().await.map_err(classify)?;
    let session = proxy.create_session(Default::default()).await.map_err(classify)?;
    let types: ashpd::enumflags2::BitFlags<SourceType> = match request.mode {
        SourceMode::Window => SourceType::Window.into(),
        SourceMode::Fullscreen | SourceMode::Area => SourceType::Monitor.into(),
    };
    let cursor_modes = proxy.available_cursor_modes().await.unwrap_or_default();
    let cursor = if cursor_modes.contains(CursorMode::Embedded) { CursorMode::Embedded } else { CursorMode::Hidden };
    let options = SelectSourcesOptions::default()
        .set_multiple(false)
        .set_sources(types)
        .set_cursor_mode(cursor)
        .set_persist_mode(PersistMode::ExplicitlyRevoked)
        .set_restore_token(request.restore_token.as_deref());
    proxy.select_sources(&session, options).await.map_err(classify)?.response().map_err(classify)?;
    let streams =
        proxy.start(&session, parent, Default::default()).await.map_err(classify)?.response().map_err(classify)?;
    let stream = streams
        .streams()
        .first()
        .cloned()
        .ok_or_else(|| StartError::Failed("The desktop shared no screen stream.".into()))?;
    let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await.map_err(classify)?;
    Ok(Source::Portal { session, stream, fd, restore_token: streams.restore_token().map(str::to_owned) })
}

/// The pixel crop for an area of a stream whose logical placement is known.
/// Returns (left, top, right, bottom) with an even resulting size.
pub fn crop_for_area(
    area: Rect,
    stream_position: (f64, f64),
    stream_logical_size: (f64, f64),
    pixel_size: (i64, i64),
) -> Option<(i64, i64, i64, i64)> {
    let (px_w, px_h) = pixel_size;
    let (log_w, log_h) = stream_logical_size;
    if px_w <= 0 || px_h <= 0 || log_w <= 0.0 || log_h <= 0.0 {
        return None;
    }
    let scale_x = px_w as f64 / log_w;
    let scale_y = px_h as f64 / log_h;
    let left = ((area.min_x() - stream_position.0) * scale_x).round().clamp(0.0, px_w as f64) as i64;
    let top = ((area.min_y() - stream_position.1) * scale_y).round().clamp(0.0, px_h as f64) as i64;
    let right_edge = ((area.max_x() - stream_position.0) * scale_x).round().clamp(0.0, px_w as f64) as i64;
    let bottom_edge = ((area.max_y() - stream_position.1) * scale_y).round().clamp(0.0, px_h as f64) as i64;
    let mut width = right_edge - left;
    let mut height = bottom_edge - top;
    if width < 16 || height < 16 {
        return None;
    }
    width -= width % 2;
    height -= height % 2;
    Some((left, top, px_w - left - width, px_h - top - height))
}

impl Recorder {
    /// Negotiates the source and starts writing. Resolves once the first
    /// frame has reached the file, so the caller's clock starts with the footage.
    pub async fn start(
        parent: Option<&WindowIdentifier>,
        request: RecordingRequest,
    ) -> Result<(Recorder, Warnings), StartError> {
        media::init().map_err(StartError::Unavailable)?;
        let mut warnings = Vec::new();
        let source = match uses_fixture() {
            Some(fixture) => Source::Fixture(fixture),
            None => {
                if gst::ElementFactory::find("pipewiresrc").is_none() {
                    return Err(StartError::Unavailable("GStreamer's pipewiresrc element is not installed.".into()));
                }
                portal_source(parent, &request).await?
            }
        };
        let Some(encoder) = video_encoder(request.frame_rate) else {
            if let Source::Portal { session, .. } = &source {
                let _ = session.close().await;
            }
            return Err(StartError::Unavailable(
                "No H.264 encoder is installed. Install GStreamer's x264 or openh264 plugin.".into(),
            ));
        };
        let fps = request.frame_rate.clamp(1, 240);
        let testing = matches!(source, Source::Fixture(_));

        let source_description = match &source {
            Source::Portal { stream, fd, .. } => format!(
                "pipewiresrc fd={} path={} keepalive-time=500 do-timestamp=true",
                fd.as_raw_fd(),
                stream.pipe_wire_node_id()
            ),
            Source::Fixture(path) => {
                format!(
                    "filesrc location=\"{}\" ! pngdec ! imagefreeze is-live=true",
                    media::escape(&path.to_string_lossy())
                )
            }
        };
        let mut description = format!(
            "{source_description} ! queue name=screenqueue ! videoconvert ! videocrop name=crop ! videorate ! video/x-raw,framerate={fps}/1 ! videoconvert ! video/x-raw,format=I420 ! {encoder} name=screenenc ! h264parse ! qtmux name=mux fragment-duration=1000 fragment-mode=first-moov-then-finalise ! filesink location=\"{}\"",
            media::escape(&request.screen_output.to_string_lossy())
        );

        let audio_encoder = audio_encoder();
        let mut includes_system_audio = false;
        let mut includes_microphone = false;
        if request.system_audio || request.microphone.is_some() {
            match audio_encoder {
                None => warnings.push("No AAC or Opus encoder is installed, so this recording has no audio.".into()),
                Some(encoder) => {
                    if request.system_audio {
                        let monitor = if testing {
                            Some("audiotestsrc is-live=true wave=silence".to_string())
                        } else {
                            crate::devices::system_audio_monitor()
                                .map(|device| format!("pulsesrc device=\"{}\"", media::escape(&device.id)))
                        };
                        match monitor {
                            Some(src) => {
                                description.push_str(&format!(
                                    " {src} ! queue ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=2 ! {encoder} ! mux."
                                ));
                                includes_system_audio = true;
                            }
                            None => warnings.push(
                                "No audio output monitor was found, so this recording will not include what the computer plays."
                                    .into(),
                            ),
                        }
                    }
                    if let Some(microphone) = &request.microphone {
                        let src = if testing {
                            "audiotestsrc is-live=true wave=silence".to_string()
                        } else {
                            format!("pulsesrc device=\"{}\"", media::escape(microphone))
                        };
                        description.push_str(&format!(
                            " {src} ! queue ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=1 ! {encoder} ! mux."
                        ));
                        includes_microphone = true;
                    }
                }
            }
        }

        let mut includes_camera = false;
        if let (Some(camera), Some(output)) = (&request.camera, &request.camera_output) {
            let src = if testing {
                "videotestsrc is-live=true pattern=ball".to_string()
            } else {
                format!("v4l2src device=\"{}\"", media::escape(camera))
            };
            description.push_str(&format!(
                " {src} ! queue ! videoconvert ! videorate ! video/x-raw,framerate={fps}/1 ! videoconvert ! video/x-raw,format=I420 ! {encoder} name=cameraenc ! h264parse ! qtmux name=cameramux fragment-duration=1000 fragment-mode=first-moov-then-finalise ! filesink location=\"{}\"",
                media::escape(&output.to_string_lossy())
            ));
            includes_camera = true;
        }

        let pipeline = match gst::parse::launch(&description) {
            Ok(element) => {
                element.downcast::<gst::Pipeline>().map_err(|_| StartError::Failed("not a pipeline".into()))?
            }
            Err(error) => {
                if let Source::Portal { session, .. } = &source {
                    let _ = session.close().await;
                }
                return Err(StartError::Failed(format!("The recording pipeline could not be built: {error}")));
            }
        };

        // Crop the chosen area once the stream's pixel size is known.
        let crop = pipeline.by_name("crop").ok_or_else(|| StartError::Failed("no crop element".into()))?;
        let (stream_position, stream_size) = match &source {
            Source::Portal { stream, .. } => (
                stream.position().map(|(x, y)| (f64::from(x), f64::from(y))),
                stream.size().map(|(w, h)| (f64::from(w), f64::from(h))),
            ),
            Source::Fixture(_) => (Some((0.0, 0.0)), None),
        };
        let area = (request.mode == SourceMode::Area).then_some(request.area).flatten();
        let pixel_scale = Arc::new(Mutex::new(1.0_f64));
        {
            let crop = crop.clone();
            let pixel_scale = pixel_scale.clone();
            let sink = crop.static_pad("sink").ok_or_else(|| StartError::Failed("no crop pad".into()))?;
            sink.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
                if let Some(gst::PadProbeData::Event(event)) = &info.data {
                    if let gst::EventView::Caps(caps) = event.view() {
                        if let Some(structure) = caps.caps().structure(0) {
                            let width = i64::from(structure.get::<i32>("width").unwrap_or(0));
                            let height = i64::from(structure.get::<i32>("height").unwrap_or(0));
                            let logical = stream_size.unwrap_or((width as f64, height as f64));
                            if logical.0 > 0.0 {
                                *pixel_scale.lock().unwrap_or_else(|e| e.into_inner()) = width as f64 / logical.0;
                            }
                            if let Some(area) = area {
                                let position = stream_position.unwrap_or((0.0, 0.0));
                                if let Some((left, top, right, bottom)) =
                                    crop_for_area(area, position, logical, (width, height))
                                {
                                    crop.set_property("left", left as i32);
                                    crop.set_property("top", top as i32);
                                    crop.set_property("right", right as i32);
                                    crop.set_property("bottom", bottom as i32);
                                }
                            }
                        }
                    }
                }
                gst::PadProbeReturn::Ok
            });
        }

        // The first frame's arrival starts the caller's clock; camera lead-in
        // is the gap between the two first frames on the shared pipeline clock.
        let timing = Arc::new(Mutex::new(Timing::default()));
        let (first_frame_sender, first_frame) = async_channel::bounded::<()>(1);
        for (name, is_camera) in [("screenenc", false), ("cameraenc", true)] {
            let Some(encoder) = pipeline.by_name(name) else { continue };
            let Some(pad) = encoder.static_pad("sink") else { continue };
            let timing = timing.clone();
            let sender = first_frame_sender.clone();
            pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                if let Some(gst::PadProbeData::Buffer(buffer)) = &info.data {
                    let mut timing = timing.lock().unwrap_or_else(|e| e.into_inner());
                    let slot = if is_camera { &mut timing.first_camera_pts } else { &mut timing.first_screen_pts };
                    if slot.is_none() {
                        *slot = buffer.pts();
                        if !is_camera {
                            let _ = sender.try_send(());
                        }
                    }
                }
                gst::PadProbeReturn::Ok
            });
        }

        let bus = pipeline.bus().ok_or_else(|| StartError::Failed("no bus".into()))?;
        let (bus_sender, bus_events) = async_channel::unbounded();
        let watch = bus
            .add_watch_local(move |_, message| {
                match message.view() {
                    gst::MessageView::Eos(_) => {
                        let _ = bus_sender.try_send(BusEvent::Eos);
                    }
                    gst::MessageView::Error(error) => {
                        let _ = bus_sender.try_send(BusEvent::Error(format!(
                            "{} ({})",
                            error.error(),
                            error.debug().unwrap_or_default()
                        )));
                    }
                    _ => {}
                }
                gtk::glib::ControlFlow::Continue
            })
            .map_err(|e| StartError::Failed(e.to_string()))?;

        if let Err(error) = pipeline.set_state(gst::State::Playing) {
            pipeline.set_state(gst::State::Null).ok();
            if let Source::Portal { session, .. } = &source {
                let _ = session.close().await;
            }
            return Err(StartError::Failed(format!("The recording could not start: {error}")));
        }

        let mut recorder = Recorder {
            pipeline,
            source,
            request,
            includes_system_audio,
            includes_microphone,
            includes_camera,
            pixel_scale: 1.0,
            timing,
            bus_events,
            _watch: watch,
            error: None,
        };

        // Wait for the first frame, or an error, for a bounded time.
        let started = Instant::now();
        loop {
            if first_frame.try_recv().is_ok() {
                break;
            }
            if let Ok(BusEvent::Error(error)) = recorder.bus_events.try_recv() {
                recorder.abandon().await;
                return Err(StartError::Failed(format!("The recording could not start: {error}")));
            }
            if started.elapsed() > Duration::from_secs(15) {
                recorder.abandon().await;
                return Err(StartError::Failed("No frames arrived from the screen.".into()));
            }
            gtk::glib::timeout_future(Duration::from_millis(50)).await;
        }
        recorder.pixel_scale = *pixel_scale.lock().unwrap_or_else(|e| e.into_inner());
        Ok((recorder, warnings))
    }

    /// An error the pipeline reported since the last check. The footage so
    /// far stays on disk; the caller should stop and keep it.
    pub fn take_error(&mut self) -> Option<String> {
        while let Ok(event) = self.bus_events.try_recv() {
            if let BusEvent::Error(error) = event {
                self.error.get_or_insert(error);
            }
        }
        self.error.clone()
    }

    /// Holds the live sources; running time stands still, so the movie has
    /// no gap when recording resumes.
    pub fn pause(&self) -> Result<(), String> {
        self.pipeline.set_state(gst::State::Paused).map(|_| ()).map_err(|e| e.to_string())
    }

    pub fn resume(&self) -> Result<(), String> {
        self.pipeline.set_state(gst::State::Playing).map(|_| ()).map_err(|e| e.to_string())
    }

    /// Finishes the movie and reads back what was written.
    pub async fn stop(mut self) -> Finished {
        let error = self.take_error();
        // Resume first so a paused pipeline lets EOS through.
        let _ = self.pipeline.set_state(gst::State::Playing);
        self.pipeline.send_event(gst::event::Eos::new());
        if matches!(self.source, Source::Fixture(_)) {
            // `imagefreeze` is not a source element, so the pipeline's EOS
            // never reaches it; end its branch from the queue after it.
            if let Some(queue) = self.pipeline.by_name("screenqueue") {
                queue.send_event(gst::event::Eos::new());
            }
        }
        let started = Instant::now();
        loop {
            match self.bus_events.try_recv() {
                Ok(BusEvent::Eos) => break,
                Ok(BusEvent::Error(message)) => {
                    self.error.get_or_insert(message);
                    break;
                }
                Err(_) => {}
            }
            if started.elapsed() > Duration::from_secs(10) {
                break;
            }
            gtk::glib::timeout_future(Duration::from_millis(50)).await;
        }
        let _ = self.pipeline.set_state(gst::State::Null);
        let restore_token = match &self.source {
            Source::Portal { session, restore_token, .. } => {
                let _ = session.close().await;
                restore_token.clone()
            }
            Source::Fixture(_) => None,
        };
        let info = media::probe(&self.request.screen_output).ok();
        let (duration, width, height) =
            info.as_ref().map(|info| (info.duration, info.width, info.height)).unwrap_or((0.0, 0, 0));
        let camera = if self.includes_camera {
            let timing = self.timing.lock().unwrap_or_else(|e| e.into_inner());
            let lead_in = match (timing.first_screen_pts, timing.first_camera_pts) {
                (Some(screen), Some(camera)) => camera.seconds_f64() - screen.seconds_f64(),
                _ => 0.0,
            };
            self.request.camera_output.as_deref().and_then(|path| media::probe(path).ok()).map(|info| CameraResult {
                lead_in,
                width: info.width,
                height: info.height,
            })
        } else {
            None
        };
        Finished {
            duration,
            width,
            height,
            pixel_scale: self.pixel_scale,
            includes_system_audio: self.includes_system_audio,
            includes_microphone: self.includes_microphone,
            camera,
            restore_token,
            error: self.error.clone().or(error),
            file_is_usable: info.as_ref().is_some_and(|info| info.playable().is_some()),
        }
    }

    /// Tears everything down without finishing the file (a failed start).
    async fn abandon(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        if let Source::Portal { session, .. } = &self.source {
            let _ = session.close().await;
        }
    }

    pub fn screen_output(&self) -> &Path {
        &self.request.screen_output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_areas_in_pixels_with_even_sizes() {
        // A 2x stream at logical (100, 0) 1280x800 → 2560x1600 pixels.
        let area = Rect::new(150.0, 20.0, 301.0, 200.5);
        let crop = crop_for_area(area, (100.0, 0.0), (1280.0, 800.0), (2560, 1600)).unwrap();
        assert_eq!(crop, (100, 40, 2560 - 100 - 602, 1600 - 40 - 400));
        assert_eq!(crop_for_area(Rect::new(0.0, 0.0, 10.0, 10.0), (0.0, 0.0), (1280.0, 800.0), (1280, 800)), None);
        assert_eq!(crop_for_area(area, (0.0, 0.0), (0.0, 0.0), (1280, 800)), None);
        // An area partly outside the stream is clamped to it.
        let clamped = crop_for_area(Rect::new(-50.0, -50.0, 100.0, 100.0), (0.0, 0.0), (1280.0, 800.0), (1280, 800));
        assert_eq!(clamped, Some((0, 0, 1230, 750)));
    }

    #[test]
    fn records_a_fixture_with_pause_and_remux() {
        if std::env::var_os("BETTERSHOT_FAKE_SCREEN").is_none() || media::init().is_err() {
            eprintln!("skipped: set BETTERSHOT_TESTING=1 and BETTERSHOT_FAKE_SCREEN to a PNG");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let request = RecordingRequest {
            mode: SourceMode::Area,
            area: Some(Rect::new(100.0, 100.0, 401.0, 301.0)),
            screen_output: dir.path().join("screen.mov"),
            camera_output: Some(dir.path().join("camera.mov")),
            frame_rate: 30,
            system_audio: true,
            microphone: Some("mic".into()),
            camera: Some("cam".into()),
            restore_token: None,
        };
        let finished = gtk::glib::MainContext::default().block_on(async {
            let (recorder, warnings) = Recorder::start(None, request).await.expect("recording starts");
            assert!(warnings.is_empty(), "{warnings:?}");
            gtk::glib::timeout_future(Duration::from_millis(1200)).await;
            recorder.pause().unwrap();
            gtk::glib::timeout_future(Duration::from_millis(1500)).await;
            recorder.resume().unwrap();
            gtk::glib::timeout_future(Duration::from_millis(1200)).await;
            recorder.stop().await
        });
        assert!(finished.file_is_usable, "{finished:?}");
        assert_eq!(finished.error, None);
        assert_eq!((finished.width, finished.height), (400, 300), "the area was cropped to an even size");
        assert!(
            finished.duration > 1.6 && finished.duration < 3.4,
            "the pause is not in the movie: {}s",
            finished.duration
        );
        assert!(finished.includes_system_audio && finished.includes_microphone);
        let camera = finished.camera.expect("camera recorded");
        assert!(camera.lead_in.abs() < 1.5, "lead-in {}", camera.lead_in);
        let info = media::probe(dir.path().join("screen.mov").as_path()).unwrap();
        assert_eq!(info.audio_streams, 2, "system audio and the microphone are separate tracks");

        let saved = dir.path().join("Saved.mp4");
        media::write_video(dir.path().join("screen.mov").as_path(), &saved).unwrap();
        let remuxed = media::probe(&saved).unwrap();
        assert_eq!((remuxed.width, remuxed.height, remuxed.audio_streams), (400, 300, 2));
        assert!((remuxed.duration - info.duration).abs() < 0.2);
        let poster = media::poster_frame(&saved, 200).unwrap();
        assert_eq!(poster.dimensions(), (200, 150));
        assert!(!dir.path().read_dir().unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".part")));
    }
}
