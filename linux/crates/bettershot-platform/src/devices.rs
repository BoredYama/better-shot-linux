//! Microphones, system-audio monitors, and cameras GStreamer can record from,
//! listed through its device providers (PulseAudio/PipeWire and V4L2).

use crate::media;
use gst::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    /// The PulseAudio source name, for `pulsesrc device=`.
    pub id: String,
    pub name: String,
    /// A sink monitor: what the computer plays.
    pub is_monitor: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraDevice {
    /// The V4L2 device path, for `v4l2src device=`.
    pub id: String,
    pub name: String,
}

fn devices(class: &str) -> Vec<gst::Device> {
    if media::init().is_err() {
        return Vec::new();
    }
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some(class), None);
    if monitor.start().is_err() {
        return Vec::new();
    }
    let devices: Vec<gst::Device> = monitor.devices().into_iter().collect();
    monitor.stop();
    devices
}

fn string_property(element: &gst::Element, name: &str) -> Option<String> {
    element.find_property(name)?;
    element.property_value(name).get::<Option<String>>().ok().flatten()
}

/// Microphones first, then monitors, each group with the default first.
pub fn audio_sources() -> Vec<AudioDevice> {
    let mut sources: Vec<AudioDevice> = devices("Audio/Source")
        .into_iter()
        .filter_map(|device| {
            let element = device.create_element(None).ok()?;
            let id = string_property(&element, "device")?;
            let properties = device.properties();
            let class = properties.as_ref().and_then(|p| p.get::<String>("device.class").ok()).unwrap_or_default();
            let is_default = properties.as_ref().and_then(|p| p.get::<bool>("is-default").ok()).unwrap_or(false);
            Some(AudioDevice {
                is_monitor: class == "monitor" || id.ends_with(".monitor"),
                name: device.display_name().to_string(),
                id,
                is_default,
            })
        })
        .collect();
    sources.sort_by_key(|d| (d.is_monitor, !d.is_default));
    sources.dedup_by(|a, b| a.id == b.id);
    sources
}

/// The monitor that carries what the computer plays, when any does.
pub fn system_audio_monitor() -> Option<AudioDevice> {
    let sources = audio_sources();
    sources.iter().find(|d| d.is_monitor && d.is_default).or_else(|| sources.iter().find(|d| d.is_monitor)).cloned()
}

pub fn cameras() -> Vec<CameraDevice> {
    let mut cameras: Vec<CameraDevice> = devices("Video/Source")
        .into_iter()
        .filter_map(|device| {
            let element = device.create_element(None).ok()?;
            if element.factory()?.name() != "v4l2src" {
                return None;
            }
            let id = string_property(&element, "device")?;
            Some(CameraDevice { id, name: device.display_name().to_string() })
        })
        .collect();
    cameras.sort_by(|a, b| a.id.cmp(&b.id));
    cameras.dedup_by(|a, b| a.id == b.id);
    cameras
}
