//! Silent still frames through the ScreenCast portal.
//!
//! The Screenshot portal's GNOME backend takes every non-interactive shot
//! through `org.gnome.Shell.Screenshot` with its flash turned on, and the
//! portal offers no option to turn it off, so each frozen frame, scrolling
//! frame, Capture Text, and loupe pick blinked the whole screen there. A
//! ScreenCast session instead streams the shared screens over PipeWire;
//! this module reads those streams through GStreamer and hands out the
//! latest frame on demand, with no flash. The desktop asks once which
//! screens to share, and the restore token keeps later sessions silent.
//! `compose` lays the streams out in the desktop's logical coordinates, the
//! same `FrozenFrame` the selector shows for a portal screenshot.

use crate::media;
use crate::screenshot::{classify, CaptureError};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use bettershot_core::geometry::Rect;
use bettershot_core::imaging::FrozenFrame;
use gst::prelude::*;
use image::RgbaImage;
use std::cell::RefCell;
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::{Duration, Instant};

/// Where a stream sits in the desktop's logical layout, as the portal
/// reports it. Backends may omit either; `compose` then infers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Placement {
    pub position: Option<(i32, i32)>,
    pub size: Option<(i32, i32)>,
}

/// The latest picture of one shared screen.
#[derive(Debug, Clone)]
pub struct StreamFrame {
    pub placement: Placement,
    pub image: RgbaImage,
}

struct Reader {
    placement: Placement,
    sink: gst_app::AppSink,
    /// PipeWire sends a frame only when the screen changes, so the last one
    /// stays current until the next arrives.
    last: RefCell<Option<RgbaImage>>,
}

/// An open ScreenCast session with a reader per shared screen.
pub struct ScreenStream {
    session: Option<Session<Screencast>>,
    _fd: Option<OwnedFd>,
    pipeline: gst::Pipeline,
    readers: Vec<Reader>,
    restore_token: Option<String>,
}

impl ScreenStream {
    /// Asks the desktop to share its screens (silently when `restore_token`
    /// is still valid) and waits for the first frame of each.
    pub async fn open(restore_token: Option<&str>) -> Result<Self, CaptureError> {
        media::init().map_err(CaptureError::Failed)?;
        let proxy = Screencast::new().await.map_err(classify)?;
        let session = proxy.create_session(Default::default()).await.map_err(classify)?;
        let cursor_modes = proxy.available_cursor_modes().await.unwrap_or_default();
        let cursor = if cursor_modes.contains(CursorMode::Hidden) { CursorMode::Hidden } else { CursorMode::Embedded };
        let options = SelectSourcesOptions::default()
            .set_multiple(true)
            .set_sources(ashpd::enumflags2::BitFlags::from(SourceType::Monitor))
            .set_cursor_mode(cursor)
            .set_persist_mode(PersistMode::ExplicitlyRevoked)
            .set_restore_token(restore_token);
        proxy.select_sources(&session, options).await.map_err(classify)?.response().map_err(classify)?;
        let streams =
            proxy.start(&session, None, Default::default()).await.map_err(classify)?.response().map_err(classify)?;
        if streams.streams().is_empty() {
            let _ = session.close().await;
            return Err(CaptureError::Failed("The desktop shared no screen.".into()));
        }
        let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await.map_err(classify)?;
        let sources: Vec<(Placement, String)> = streams
            .streams()
            .iter()
            .map(|stream| {
                let placement = Placement { position: stream.position(), size: stream.size() };
                let source = format!(
                    "pipewiresrc fd={} path={} keepalive-time=500 do-timestamp=true",
                    fd.as_raw_fd(),
                    stream.pipe_wire_node_id()
                );
                (placement, source)
            })
            .collect();
        let mut stream = Self::from_sources(sources).map_err(CaptureError::Failed)?;
        stream.session = Some(session);
        stream._fd = Some(fd);
        stream.restore_token = streams.restore_token().map(str::to_owned);
        Ok(stream)
    }

    /// Reads GStreamer source descriptions directly (tests play a fixture
    /// through `imagefreeze`). The pipeline starts, but no frame is awaited.
    pub fn from_sources(sources: Vec<(Placement, String)>) -> Result<Self, String> {
        media::init()?;
        let description: Vec<String> = sources
            .iter()
            .enumerate()
            .map(|(index, (_, source))| {
                format!(
                    "{source} ! queue max-size-buffers=1 leaky=downstream ! videoconvert ! video/x-raw,format=RGBA ! appsink name=sink{index} sync=false max-buffers=1 drop=true"
                )
            })
            .collect();
        let pipeline = gst::parse::launch(&description.join(" "))
            .map_err(|e| e.to_string())?
            .downcast::<gst::Pipeline>()
            .map_err(|_| "not a pipeline".to_string())?;
        let readers = sources
            .iter()
            .enumerate()
            .map(|(index, (placement, _))| {
                let sink = pipeline
                    .by_name(&format!("sink{index}"))
                    .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
                    .ok_or("no appsink")?;
                Ok(Reader { placement: *placement, sink, last: RefCell::new(None) })
            })
            .collect::<Result<Vec<_>, String>>()?;
        pipeline.set_state(gst::State::Playing).map_err(|e| e.to_string())?;
        Ok(Self { session: None, _fd: None, pipeline, readers, restore_token: None })
    }

    pub fn restore_token(&self) -> Option<&str> {
        self.restore_token.as_deref()
    }

    /// The latest frame of every shared screen. Waits up to `timeout` for a
    /// screen that has not sent one yet; a screen that has sent one before
    /// and is unchanged returns that frame again.
    pub fn frames(&self, timeout: Duration) -> Result<Vec<StreamFrame>, String> {
        let deadline = Instant::now() + timeout;
        let mut frames = Vec::with_capacity(self.readers.len());
        for reader in &self.readers {
            let mut newest = None;
            while let Some(sample) = reader.sink.try_pull_sample(gst::ClockTime::ZERO) {
                newest = Some(sample);
            }
            if newest.is_none() && reader.last.borrow().is_none() {
                loop {
                    self.check_errors()?;
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err("The shared screen sent no picture.".into());
                    }
                    let slice = remaining.min(Duration::from_millis(100));
                    if let Some(sample) =
                        reader.sink.try_pull_sample(gst::ClockTime::from_nseconds(slice.as_nanos() as u64))
                    {
                        newest = Some(sample);
                        break;
                    }
                }
            }
            if let Some(sample) = newest {
                *reader.last.borrow_mut() = Some(image_from_sample(&sample)?);
            }
            self.check_errors()?;
            let image = reader.last.borrow().clone().ok_or("The shared screen sent no picture.")?;
            frames.push(StreamFrame { placement: reader.placement, image });
        }
        Ok(frames)
    }

    fn check_errors(&self) -> Result<(), String> {
        let Some(bus) = self.pipeline.bus() else { return Ok(()) };
        if let Some(message) = bus.pop_filtered(&[gst::MessageType::Error]) {
            if let gst::MessageView::Error(error) = message.view() {
                return Err(format!("The screen stream stopped: {}", error.error()));
            }
        }
        Ok(())
    }

    /// Stops reading and ends the portal session, which clears the
    /// desktop's sharing indicator.
    pub async fn close(&self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        if let Some(session) = &self.session {
            let _ = session.close().await;
        }
    }
}

impl Drop for ScreenStream {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn image_from_sample(sample: &gst::Sample) -> Result<RgbaImage, String> {
    let caps = sample.caps().ok_or("a frame without caps")?;
    let structure = caps.structure(0).ok_or("a frame without a format")?;
    let width = structure.get::<i32>("width").map_err(|e| e.to_string())?.max(0) as u32;
    let height = structure.get::<i32>("height").map_err(|e| e.to_string())?.max(0) as u32;
    let buffer = sample.buffer().ok_or("a frame without pixels")?;
    let map = buffer.map_readable().map_err(|e| e.to_string())?;
    let data = map.as_slice();
    let row = width as usize * 4;
    if width == 0 || height == 0 || data.len() < row * height as usize {
        return Err(format!("a {width}x{height} frame with {} bytes", data.len()));
    }
    let stride = data.len() / height as usize;
    let mut pixels = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        pixels.extend_from_slice(&data[y * stride..y * stride + row]);
    }
    RgbaImage::from_raw(width, height, pixels).ok_or_else(|| "a frame that does not fit its size".to_string())
}

/// Lays the shared screens out as one frame in the desktop's logical
/// coordinates (`bounds` is the box around `monitors`), at the largest pixel
/// scale among them. A stream without a reported placement takes the
/// monitor with its logical size, or the whole desktop when it is alone.
pub fn compose(frames: &[StreamFrame], monitors: &[Rect], bounds: Rect) -> Result<FrozenFrame, String> {
    if frames.is_empty() {
        return Err("No screen was shared.".into());
    }
    let mut placed: Vec<(Rect, &RgbaImage)> = Vec::with_capacity(frames.len());
    for frame in frames {
        let logical = logical_rect(frame, monitors, bounds, frames.len())?;
        placed.push((logical, &frame.image));
    }
    let scale =
        placed.iter().map(|(rect, image)| f64::from(image.width()) / rect.width.max(1.0)).fold(1.0_f64, f64::max);
    let canvas_width = (bounds.width * scale).round().max(1.0) as u32;
    let canvas_height = (bounds.height * scale).round().max(1.0) as u32;
    let mut canvas = RgbaImage::from_pixel(canvas_width, canvas_height, image::Rgba([0, 0, 0, 255]));
    for (rect, image) in placed {
        let target_width = (rect.width * scale).round().max(1.0) as u32;
        let target_height = (rect.height * scale).round().max(1.0) as u32;
        let x = ((rect.x - bounds.x) * scale).round() as i64;
        let y = ((rect.y - bounds.y) * scale).round() as i64;
        if (image.width(), image.height()) == (target_width, target_height) {
            image::imageops::replace(&mut canvas, image, x, y);
        } else {
            let resized =
                image::imageops::resize(image, target_width, target_height, image::imageops::FilterType::Triangle);
            image::imageops::replace(&mut canvas, &resized, x, y);
        }
    }
    Ok(FrozenFrame { points_rect: bounds, image: canvas })
}

fn logical_rect(frame: &StreamFrame, monitors: &[Rect], bounds: Rect, count: usize) -> Result<Rect, String> {
    let size = frame.placement.size.map(|(w, h)| (f64::from(w), f64::from(h)));
    if let (Some((x, y)), Some((width, height))) = (frame.placement.position, size) {
        if width > 0.0 && height > 0.0 {
            return Ok(Rect::new(f64::from(x), f64::from(y), width, height));
        }
    }
    if let Some((width, height)) = size {
        let mut matching = monitors.iter().filter(|m| m.width == width && m.height == height);
        if let (Some(monitor), None) = (matching.next(), matching.next()) {
            return Ok(*monitor);
        }
    }
    if count == 1 {
        return Ok(if monitors.len() == 1 { monitors[0] } else { bounds });
    }
    Err("The desktop did not say where a shared screen is.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: i32, y: i32, w: i32, h: i32, pw: u32, ph: u32, color: [u8; 4]) -> StreamFrame {
        StreamFrame {
            placement: Placement { position: Some((x, y)), size: Some((w, h)) },
            image: RgbaImage::from_pixel(pw, ph, image::Rgba(color)),
        }
    }

    #[test]
    fn composes_streams_at_the_largest_scale() {
        // A 2x 100x50 monitor beside a 1x 100x50 one.
        let monitors = [Rect::new(0.0, 0.0, 100.0, 50.0), Rect::new(100.0, 0.0, 100.0, 50.0)];
        let bounds = Rect::new(0.0, 0.0, 200.0, 50.0);
        let left = frame(0, 0, 100, 50, 200, 100, [255, 0, 0, 255]);
        let right = frame(100, 0, 100, 50, 100, 50, [0, 0, 255, 255]);
        let composed = compose(&[left, right], &monitors, bounds).unwrap();
        assert_eq!((composed.image.width(), composed.image.height()), (400, 100));
        assert_eq!(composed.points_rect, bounds);
        assert_eq!(composed.image.get_pixel(10, 10).0, [255, 0, 0, 255]);
        assert_eq!(composed.image.get_pixel(390, 90).0, [0, 0, 255, 255], "the 1x stream was scaled up");
        // The selector's crop math works on the composed frame.
        let crop = composed.cropped(Rect::new(150.0, 0.0, 50.0, 50.0)).unwrap();
        assert_eq!((crop.width(), crop.height()), (100, 100));
    }

    #[test]
    fn infers_missing_placements() {
        let monitors = [Rect::new(0.0, 0.0, 100.0, 50.0), Rect::new(100.0, 0.0, 60.0, 50.0)];
        let bounds = Rect::new(0.0, 0.0, 160.0, 50.0);
        let mut unplaced = frame(0, 0, 60, 50, 60, 50, [0, 255, 0, 255]);
        unplaced.placement.position = None;
        let composed = compose(&[unplaced.clone()], &monitors, bounds).unwrap();
        assert_eq!(composed.image.get_pixel(120, 10).0, [0, 255, 0, 255], "matched the monitor with its size");
        assert_eq!(composed.image.get_pixel(10, 10).0, [0, 0, 0, 255], "the unshared monitor is black");
        unplaced.placement = Placement::default();
        let whole = compose(&[unplaced.clone()], &monitors, bounds).unwrap();
        assert_eq!((whole.image.width(), whole.image.height()), (160, 50), "a lone stream covers the desktop");
        assert!(compose(&[unplaced.clone(), unplaced], &monitors, bounds).is_err());
        assert!(compose(&[], &monitors, bounds).is_err());
    }

    #[test]
    fn reads_the_latest_frame_from_a_fixture_stream() {
        if media::init().is_err() {
            eprintln!("skipped: GStreamer is unavailable");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("screen.png");
        let mut image = RgbaImage::from_pixel(64, 48, image::Rgba([10, 20, 30, 255]));
        image.put_pixel(63, 47, image::Rgba([200, 100, 50, 255]));
        image.save(&path).unwrap();
        let source = format!(
            "filesrc location=\"{}\" ! pngdec ! imagefreeze is-live=true",
            media::escape(&path.to_string_lossy())
        );
        let placement = Placement { position: Some((0, 0)), size: Some((64, 48)) };
        let stream = ScreenStream::from_sources(vec![(placement, source)]).unwrap();
        let frames = stream.frames(Duration::from_secs(10)).expect("a frame arrives");
        assert_eq!(frames.len(), 1);
        assert_eq!((frames[0].image.width(), frames[0].image.height()), (64, 48));
        assert_eq!(frames[0].image.get_pixel(0, 0).0, [10, 20, 30, 255]);
        assert_eq!(frames[0].image.get_pixel(63, 47).0, [200, 100, 50, 255], "rows are read at the right stride");
        // Later reads return the newest queued frame, or the last one again.
        let again = stream.frames(Duration::from_millis(50)).unwrap();
        assert_eq!(again[0].image.as_raw(), frames[0].image.as_raw());
        let composed = compose(&frames, &[Rect::new(0.0, 0.0, 64.0, 48.0)], Rect::new(0.0, 0.0, 64.0, 48.0)).unwrap();
        assert_eq!(composed.image.as_raw(), frames[0].image.as_raw());
        gtk::glib::MainContext::default().block_on(stream.close());
    }
}
