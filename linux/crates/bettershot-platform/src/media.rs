//! Reading and rewriting video files with GStreamer: what a recording's
//! screen movie contains, its first frame for posters and cards, and the
//! container conversion Save needs (`VideoFileActions.save(from:to:)` on
//! macOS). Nothing here re-encodes video.

use bettershot_core::recording::PlayableMetadata;
use gst::prelude::*;
use image::RgbaImage;
use std::io;
use std::path::Path;
use std::time::Duration;

/// Initializes GStreamer once. Safe to call from any thread.
pub fn init() -> Result<(), String> {
    static RESULT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    RESULT.get_or_init(|| gst::init().map_err(|e| e.to_string())).clone()
}

/// Whether a GStreamer element is installed.
pub fn element_exists(name: &str) -> Result<bool, String> {
    init()?;
    Ok(gst::ElementFactory::find(name).is_some())
}

/// What a movie file holds.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub duration: f64,
    pub width: i64,
    pub height: i64,
    pub video_streams: usize,
    pub audio_streams: usize,
}

impl MediaInfo {
    pub fn playable(&self) -> Option<PlayableMetadata> {
        (self.video_streams > 0 && self.duration.is_finite() && self.duration > 0.0).then_some(PlayableMetadata {
            duration: self.duration,
            width: self.width,
            height: self.height,
        })
    }
}

/// Reads a movie's duration, size, and stream counts.
pub fn probe(path: &Path) -> Result<MediaInfo, String> {
    init()?;
    let uri = file_uri(path)?;
    let discoverer = gst_pbutils::Discoverer::new(gst::ClockTime::from_seconds(10)).map_err(|e| e.to_string())?;
    let info = discoverer.discover_uri(&uri).map_err(|e| format!("The video could not be read: {e}"))?;
    let duration = info.duration().map(|d| d.seconds_f64()).unwrap_or(0.0);
    let videos = info.video_streams();
    let (width, height) =
        videos.first().map(|stream| (i64::from(stream.width()), i64::from(stream.height()))).unwrap_or((0, 0));
    Ok(MediaInfo { duration, width, height, video_streams: videos.len(), audio_streams: info.audio_streams().len() })
}

/// The recovery probe: playable screen movies only.
pub fn playable_metadata(path: &Path) -> Option<PlayableMetadata> {
    probe(path).ok()?.playable()
}

/// Decodes the first frame, scaled so its longer edge is at most `max_edge`.
pub fn poster_frame(path: &Path, max_edge: u32) -> Result<RgbaImage, String> {
    init()?;
    let uri = file_uri(path)?;
    let description = format!(
        "uridecodebin uri=\"{}\" name=decode ! videoconvert ! videoscale ! video/x-raw,format=RGBA ! appsink name=sink sync=false max-buffers=1",
        escape(&uri)
    );
    let pipeline = gst::parse::launch(&description)
        .map_err(|e| e.to_string())?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "not a pipeline".to_string())?;
    let sink =
        pipeline.by_name("sink").and_then(|element| element.downcast::<gst_app::AppSink>().ok()).ok_or("no appsink")?;
    pipeline.set_state(gst::State::Paused).map_err(|e| e.to_string())?;
    let (result, _, _) = pipeline.state(gst::ClockTime::from_seconds(10));
    if result.is_err() {
        pipeline.set_state(gst::State::Null).ok();
        return Err("The video could not be decoded.".into());
    }
    let sample = sink.pull_preroll().map_err(|_| "The video has no frames.".to_string());
    let image = sample.and_then(|sample| {
        let caps = sample.caps().ok_or("no caps")?;
        let structure = caps.structure(0).ok_or("no caps")?;
        let width = structure.get::<i32>("width").map_err(|e| e.to_string())? as u32;
        let height = structure.get::<i32>("height").map_err(|e| e.to_string())? as u32;
        let buffer = sample.buffer().ok_or("no buffer")?;
        let map = buffer.map_readable().map_err(|e| e.to_string())?;
        let stride = (map.len() / height.max(1) as usize).max(width as usize * 4);
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for row in 0..height as usize {
            let start = row * stride;
            pixels.extend_from_slice(&map[start..start + width as usize * 4]);
        }
        RgbaImage::from_raw(width, height, pixels).ok_or_else(|| "frame size mismatch".to_string())
    });
    pipeline.set_state(gst::State::Null).ok();
    let image = image?;
    Ok(bettershot_core::imaging::thumbnail(&image, max_edge))
}

/// Writes the first frame as the package's `poster.jpg`.
pub fn write_poster(video: &Path, poster: &Path) -> Result<(), String> {
    let image = poster_frame(video, 640)?;
    let rgb = image::DynamicImage::ImageRgba8(image).to_rgb8();
    let mut data = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, 72);
    encoder.encode_image(&rgb).map_err(|e| e.to_string())?;
    bettershot_core::prefs::write_atomically(poster, &data).map_err(|e| e.to_string())
}

/// Writes `source` to `destination` in the destination's container (by
/// extension), staging the complete file beside it first so a failure never
/// leaves a half-written export. Same container: a plain copy.
pub fn write_video(source: &Path, destination: &Path) -> io::Result<()> {
    let parent = destination.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let extension = destination.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let staged = parent.join(format!(
        ".{}.{}.part",
        uuid::Uuid::new_v4(),
        if extension.is_empty() { "mov" } else { &extension }
    ));
    let same_container =
        source.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) == Some(extension.clone());
    let result = if same_container {
        std::fs::copy(source, &staged).map(|_| ())
    } else {
        remux(source, &staged, &extension).map_err(io::Error::other)
    };
    match result.and_then(|()| std::fs::rename(&staged, destination)) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&staged);
            Err(error)
        }
    }
}

/// Rewraps the streams of `source` into `destination` without re-encoding.
pub fn remux(source: &Path, destination: &Path, container: &str) -> Result<(), String> {
    init()?;
    let info = probe(source)?;
    let muxer = match container {
        "mp4" => "mp4mux",
        "mov" => "qtmux",
        other => return Err(format!("Videos cannot be saved as .{other}.")),
    };
    let mut description = format!(
        "filesrc location=\"{}\" ! qtdemux name=demux {muxer} name=mux ! filesink location=\"{}\"",
        escape(&source.to_string_lossy()),
        escape(&destination.to_string_lossy())
    );
    for index in 0..info.video_streams {
        description.push_str(&format!(" demux.video_{index} ! queue ! h264parse ! mux."));
    }
    for index in 0..info.audio_streams {
        description.push_str(&format!(" demux.audio_{index} ! queue ! mux."));
    }
    run_to_eos(&description, Duration::from_secs(600))
}

/// Runs a pipeline description until EOS or an error.
pub fn run_to_eos(description: &str, timeout: Duration) -> Result<(), String> {
    let pipeline = gst::parse::launch(description).map_err(|e| e.to_string())?;
    let bus = pipeline.bus().ok_or("no bus")?;
    pipeline.set_state(gst::State::Playing).map_err(|e| e.to_string())?;
    let outcome = loop {
        let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(timeout.as_millis() as u64)) else {
            break Err("The video took too long to write.".to_string());
        };
        match message.view() {
            gst::MessageView::Eos(_) => break Ok(()),
            gst::MessageView::Error(error) => {
                break Err(format!("{} ({})", error.error(), error.debug().unwrap_or_default()));
            }
            _ => {}
        }
    };
    pipeline.set_state(gst::State::Null).ok();
    outcome
}

pub fn file_uri(path: &Path) -> Result<String, String> {
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    url::Url::from_file_path(&absolute).map(|url| url.to_string()).map_err(|_| "not an absolute path".to_string())
}

/// Quotes a value for a parse-launch description.
pub fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
