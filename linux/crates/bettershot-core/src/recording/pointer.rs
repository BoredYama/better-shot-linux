//! Port of `RecordingPointerStream.swift`, `RecordingPointerTimeline.swift`,
//! and the click feedback in `RecordingOverlayEffects.swift`: the pointer
//! capture sidecar is cleaned once into one ordered stream, then rebuilt
//! into a deterministic, fixed-step timeline of where the synthetic cursor
//! is drawn, how big it is, and which press pulse is showing. Source events
//! are remapped through the clip edit before the springs integrate them,
//! so the preview and the export interpolate the same edited-time frames.

use super::clips::ClipTimeline;
use super::edit::{CursorAppearance, CursorOptions};
use super::manifest::{
    ArtworkPoint, ArtworkSize, PointerArtwork, PointerCaptureFile, PointerPressEvent, PointerTravelKind,
    PointerTravelSample, PressPhase, RecordingKeystrokeEvent,
};
use super::viewport::{DampedSpring, SpringConstant, UnitPoint};
use std::collections::{HashMap, HashSet};

// MARK: - The sanitized stream

#[derive(Debug, Clone, PartialEq)]
pub struct PointerSanitizeOptions {
    /// The recording's size in points, which turns normalized distances
    /// into pixels for the spike and thinning thresholds.
    pub recording_size_in_points: (f64, f64),
    pub minimum_travel: f64,
    pub max_sample_gap: f64,
}

impl PointerSanitizeOptions {
    pub fn new(recording_size_in_points: (f64, f64)) -> Self {
        Self { recording_size_in_points, minimum_travel: 1.0, max_sample_gap: 0.25 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Travel,
    Drag,
    Press,
    Release,
}

impl StreamKind {
    fn priority(self) -> u8 {
        match self {
            Self::Travel | Self::Drag => 0,
            Self::Press => 1,
            Self::Release => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PointerStreamEvent {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    pub kind: StreamKind,
    pub button: Option<i64>,
    pub artwork_id: Option<String>,
}

impl PointerStreamEvent {
    pub fn location(&self) -> UnitPoint {
        UnitPoint::new(self.x, self.y)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PointerStream {
    pub sanitized_capture: PointerCaptureFile,
    pub samples: Vec<PointerStreamEvent>,
    pub artwork: Vec<PointerArtwork>,
}

fn clamped_unit_point(x: f64, y: f64) -> Option<(f64, f64)> {
    // A tiny tolerance absorbs floating-point conversion at the capture
    // boundary; real out-of-frame input is discarded, not pinned to an edge.
    if !x.is_finite() || !y.is_finite() || !(-0.001..=1.001).contains(&x) || !(-0.001..=1.001).contains(&y) {
        return None;
    }
    Some((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
}

fn sanitized_size(size: (f64, f64)) -> (f64, f64) {
    (
        if size.0.is_finite() && size.0 > 0.0 { size.0 } else { 1000.0 },
        if size.1.is_finite() && size.1 > 0.0 { size.1 } else { 1000.0 },
    )
}

fn distance_in_points(a: &PointerTravelSample, b: &PointerTravelSample, size: (f64, f64)) -> f64 {
    ((b.x - a.x) * size.0).hypot((b.y - a.y) * size.1)
}

fn valid_artwork(items: &[PointerArtwork]) -> Vec<PointerArtwork> {
    let mut seen = HashSet::new();
    items
        .iter()
        .filter(|artwork| {
            !artwork.artwork_id.is_empty()
                && seen.insert(artwork.artwork_id.clone())
                && !artwork.image_data.is_empty()
                && artwork.anchor_point.x.is_finite()
                && artwork.anchor_point.y.is_finite()
                && artwork.reference_size.width.is_finite()
                && artwork.reference_size.height.is_finite()
                && artwork.reference_size.width > 0.0
                && artwork.reference_size.height > 0.0
        })
        .cloned()
        .collect()
}

fn valid_artwork_id(id: Option<&str>, valid: &HashSet<String>) -> Option<String> {
    id.filter(|id| valid.contains(*id)).map(str::to_owned)
}

fn drop_isolated_spikes(samples: Vec<PointerTravelSample>, size: (f64, f64)) -> Vec<PointerTravelSample> {
    if samples.len() <= 2 {
        return samples;
    }
    let diagonal = size.0.hypot(size.1);
    let spike_floor = (diagonal * 0.24).max(160.0);
    let neighbor_ceiling = (diagonal * 0.025).max(16.0);
    let last = samples.len() - 1;
    samples
        .iter()
        .enumerate()
        .filter(|(index, sample)| {
            if *index == 0 || *index == last {
                return true;
            }
            let previous = &samples[index - 1];
            let next = &samples[index + 1];
            if sample.kind != previous.kind
                || sample.kind != next.kind
                || sample.artwork_id != previous.artwork_id
                || sample.artwork_id != next.artwork_id
            {
                return true;
            }
            let from_previous = distance_in_points(previous, sample, size);
            let to_next = distance_in_points(sample, next, size);
            let neighbors = distance_in_points(previous, next, size);
            !(from_previous > spike_floor && to_next > spike_floor && neighbors < neighbor_ceiling)
        })
        .map(|(_, sample)| sample.clone())
        .collect()
}

fn thin_travel_samples(
    samples: Vec<PointerTravelSample>,
    size: (f64, f64),
    minimum_travel: f64,
    max_sample_gap: f64,
) -> Vec<PointerTravelSample> {
    if samples.len() <= 2 {
        return samples;
    }
    let mut result = vec![samples[0].clone()];
    for sample in &samples[1..samples.len() - 1] {
        let Some(previous) = result.last() else { continue };
        let elapsed = sample.time - previous.time;
        let preserves_state = sample.kind != previous.kind || sample.artwork_id != previous.artwork_id;
        let moved = distance_in_points(previous, sample, size);
        if preserves_state || elapsed >= max_sample_gap || (elapsed >= 1.0 / 120.0 && moved >= minimum_travel) {
            result.push(sample.clone());
        }
    }
    if let Some(last) = samples.last() {
        if result.last() != Some(last) {
            result.push(last.clone());
        }
    }
    result
}

fn sanitized_keystrokes(keystrokes: &[RecordingKeystrokeEvent]) -> Vec<RecordingKeystrokeEvent> {
    let mut events: Vec<RecordingKeystrokeEvent> = keystrokes
        .iter()
        .filter(|event| event.time.is_finite() && !event.key.is_empty())
        .map(|event| RecordingKeystrokeEvent {
            time: event.time.max(0.0),
            modifiers: event.modifiers.iter().filter(|m| !m.is_empty()).cloned().collect(),
            key: event.key.clone(),
        })
        .collect();
    events.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    events
}

fn build_stream(capture: PointerCaptureFile) -> PointerStream {
    let mut ordered: Vec<(PointerStreamEvent, usize)> =
        Vec::with_capacity(capture.travel.len() + capture.presses.len());
    for (index, sample) in capture.travel.iter().enumerate() {
        if !sample.time.is_finite() || !sample.x.is_finite() || !sample.y.is_finite() {
            continue;
        }
        ordered.push((
            PointerStreamEvent {
                time: sample.time,
                x: sample.x,
                y: sample.y,
                kind: if sample.kind == PointerTravelKind::Drag { StreamKind::Drag } else { StreamKind::Travel },
                button: None,
                artwork_id: sample.artwork_id.clone(),
            },
            index * 2,
        ));
    }
    let offset = capture.travel.len() * 2;
    for (index, press) in capture.presses.iter().enumerate() {
        if !press.time.is_finite() || !press.x.is_finite() || !press.y.is_finite() {
            continue;
        }
        ordered.push((
            PointerStreamEvent {
                time: press.time,
                x: press.x,
                y: press.y,
                kind: if press.phase == PressPhase::Down { StreamKind::Press } else { StreamKind::Release },
                button: Some(press.button),
                artwork_id: press.artwork_id.clone(),
            },
            offset + index * 2 + 1,
        ));
    }
    ordered.sort_by(|a, b| {
        a.0.time
            .partial_cmp(&b.0.time)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.kind.priority().cmp(&b.0.kind.priority()))
            .then(a.1.cmp(&b.1))
    });
    let artwork = capture.artwork.clone();
    PointerStream {
        samples: ordered.into_iter().map(|(event, _)| event).collect(),
        artwork,
        sanitized_capture: capture,
    }
}

/// Cleans the sidecar once (`PointerStreamSanitizer.sanitize`): invalid and
/// out-of-frame samples go, travel is ordered, de-spiked, and thinned, and
/// everything merges into one time-ordered stream.
pub fn sanitize(capture: &PointerCaptureFile, options: &PointerSanitizeOptions) -> PointerStream {
    if capture.is_sanitized {
        return build_stream(capture.clone());
    }
    let artwork = valid_artwork(&capture.artwork);
    let valid_ids: HashSet<String> = artwork.iter().map(|a| a.artwork_id.clone()).collect();
    let size = sanitized_size(options.recording_size_in_points);

    let mut ranked: Vec<(usize, PointerTravelSample)> = capture
        .travel
        .iter()
        .enumerate()
        .filter_map(|(index, sample)| {
            if !sample.time.is_finite() {
                return None;
            }
            let (x, y) = clamped_unit_point(sample.x, sample.y)?;
            Some((
                index,
                PointerTravelSample {
                    time: sample.time.max(0.0),
                    x,
                    y,
                    kind: sample.kind,
                    artwork_id: valid_artwork_id(sample.artwork_id.as_deref(), &valid_ids),
                },
            ))
        })
        .collect();
    ranked.sort_by(|a, b| a.1.time.partial_cmp(&b.1.time).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    let ordered: Vec<PointerTravelSample> = ranked.into_iter().map(|(_, sample)| sample).collect();
    let stable = drop_isolated_spikes(ordered, size);
    let travel =
        thin_travel_samples(stable, size, options.minimum_travel.max(0.0), options.max_sample_gap.max(1.0 / 120.0));

    let mut presses: Vec<(usize, PointerPressEvent)> = capture
        .presses
        .iter()
        .enumerate()
        .filter_map(|(index, press)| {
            if !press.time.is_finite() {
                return None;
            }
            let (x, y) = clamped_unit_point(press.x, press.y)?;
            Some((
                index,
                PointerPressEvent {
                    time: press.time.max(0.0),
                    x,
                    y,
                    button: press.button.max(0),
                    phase: press.phase,
                    artwork_id: valid_artwork_id(press.artwork_id.as_deref(), &valid_ids),
                },
            ))
        })
        .collect();
    presses.sort_by(|a, b| a.1.time.partial_cmp(&b.1.time).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));

    build_stream(PointerCaptureFile {
        format_version: PointerCaptureFile::CURRENT_FORMAT,
        travel,
        presses: presses.into_iter().map(|(_, press)| press).collect(),
        keystrokes: sanitized_keystrokes(&capture.keystrokes),
        artwork,
        is_sanitized: true,
    })
}

impl PointerCaptureFile {
    pub const CURRENT_FORMAT: i64 = 1;

    /// Whether anything was recorded, so a timeline can be built.
    pub fn has_pointer_data(&self) -> bool {
        !self.travel.is_empty() || !self.presses.is_empty()
    }
}

// MARK: - Artwork metrics

/// Pointer artwork height as a fraction of the unzoomed content height.
pub const ARTWORK_HEIGHT_RATIO: f64 = 0.032;
/// Artwork ids of the editor's own cursor styles, which the renderer draws
/// as vectors instead of decoding `imageData`.
pub const STYLED_ARTWORK_PREFIX: &str = "bettershot-cursor-";

impl PointerArtwork {
    pub fn normalized_anchor(&self) -> UnitPoint {
        let width = self.reference_size.width.max(1.0);
        let height = self.reference_size.height.max(1.0);
        UnitPoint::new((self.anchor_point.x / width).clamp(0.0, 1.0), (self.anchor_point.y / height).clamp(0.0, 1.0))
    }

    pub fn aspect_ratio(&self) -> f64 {
        if self.reference_size.width > 0.0 && self.reference_size.height > 0.0 {
            self.reference_size.width / self.reference_size.height
        } else {
            1.0
        }
    }

    /// Preserves relative system-cursor sizes without letting drag artwork
    /// or malformed metadata dominate the composition.
    pub fn intrinsic_scale(&self) -> f64 {
        if self.reference_size.height > 0.0 {
            (self.reference_size.height / 16.0).clamp(0.75, 2.5)
        } else {
            1.0
        }
    }

    /// Whether this is one of the editor's vector cursor styles.
    pub fn is_styled(&self) -> bool {
        self.artwork_id.starts_with(STYLED_ARTWORK_PREFIX)
    }

    /// The cursor style a styled artwork id names.
    pub fn styled_appearance(&self) -> Option<CursorAppearance> {
        let raw = self.artwork_id.strip_prefix(STYLED_ARTWORK_PREFIX)?;
        CursorAppearance::from_raw_value(raw)
    }
}

/// Port of `PointerArtworkCapture.styledArtwork`: the editor-selected cursor
/// styles share the sidecar's artwork shape. Their pixels are drawn as
/// vectors by the renderer, so `image_data` stays empty; the logical size
/// (32×40) and hotspot match macOS, so projects agree across platforms.
pub fn styled_artwork(appearance: CursorAppearance) -> Option<PointerArtwork> {
    if appearance == CursorAppearance::Recorded {
        return None;
    }
    let hotspot = match appearance {
        CursorAppearance::Dot => (16.0, 20.0),
        CursorAppearance::Hand => (12.0, 5.0),
        _ => (5.0, 4.0),
    };
    Some(PointerArtwork {
        artwork_id: format!("{STYLED_ARTWORK_PREFIX}{}", appearance.raw_value()),
        image_data: Vec::new(),
        anchor_point: ArtworkPoint { x: hotspot.0, y: hotspot.1 },
        reference_size: ArtworkSize { width: 32.0, height: 40.0 },
    })
}

/// The cursor drawn when a sample names no recorded artwork
/// (`PointerArtworkCapture.defaultArtwork` captures the system arrow; the
/// Linux sampler records no artwork, so the vector arrow stands in).
pub fn default_artwork() -> PointerArtwork {
    styled_artwork(CursorAppearance::Arrow).unwrap_or_else(|| PointerArtwork {
        artwork_id: format!("{STYLED_ARTWORK_PREFIX}macOS"),
        image_data: Vec::new(),
        anchor_point: ArtworkPoint { x: 5.0, y: 4.0 },
        reference_size: ArtworkSize { width: 32.0, height: 40.0 },
    })
}

// MARK: - Click feedback

/// `PointerPressEffectGeometry`: one frame of the impact-and-ripple pulse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressEffectGeometry {
    pub impact_radius: f64,
    pub impact_opacity: f64,
    pub ripple_radius: f64,
    pub ripple_opacity: f64,
    pub ripple_line_width: f64,
}

/// macOS system blue.
pub const PRESS_EFFECT_COLOR: (f64, f64, f64) = (0.0, 122.0 / 255.0, 1.0);
pub const PRESS_EFFECT_DURATION: f64 = 0.4;
const IMPACT_DURATION: f64 = 0.12;
const RIPPLE_DELAY: f64 = 0.06;

fn ease_out_cubic(progress: f64) -> f64 {
    1.0 - (1.0 - progress.clamp(0.0, 1.0)).powi(3)
}

/// Shared preview/export math for a crisp impact followed by a quieter
/// ripple. `reference_height` is the unzoomed content height: camera
/// transforms move the effect but never inflate its on-screen size.
pub fn press_effect_geometry(progress: f64, reference_height: f64, cursor_scale: f64) -> PressEffectGeometry {
    let age = progress.clamp(0.0, 1.0) * PRESS_EFFECT_DURATION;
    let base = reference_height * (21.0 / 1080.0) * cursor_scale;
    let impact_progress = (age / IMPACT_DURATION).clamp(0.0, 1.0);
    let impact_ease = ease_out_cubic(impact_progress);
    let ripple_progress = ((age - RIPPLE_DELAY) / (PRESS_EFFECT_DURATION - RIPPLE_DELAY)).clamp(0.0, 1.0);
    let ripple_ease = ease_out_cubic(ripple_progress);
    PressEffectGeometry {
        impact_radius: base * (0.38 + 0.34 * impact_ease),
        impact_opacity: if age <= IMPACT_DURATION { 0.38 * (1.0 - impact_ease) } else { 0.0 },
        ripple_radius: base * (0.62 + 0.93 * ripple_ease),
        ripple_opacity: if age >= RIPPLE_DELAY { 0.44 * (1.0 - ripple_ease) } else { 0.0 },
        ripple_line_width: (base * (0.14 - 0.07 * ripple_ease)).max(1.0),
    }
}

// MARK: - The timeline

#[derive(Debug, Clone, PartialEq)]
pub struct PointerPressFrame {
    /// The exact recorded press coordinate: the smoothed cursor may still be
    /// settling when the click lands, but the feedback marks the real target.
    pub location: UnitPoint,
    pub progress: f64,
    pub impact_enabled: bool,
    pub ripple_enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PointerFrame {
    /// Normalized recording coordinate, top-left origin.
    pub location: UnitPoint,
    pub artwork_id: Option<String>,
    /// Interaction and visibility magnification, anchored at the artwork's anchor.
    pub magnification: f64,
    pub tilt_degrees: f64,
    pub opacity: f64,
    pub blur_radius: f64,
    pub press: Option<PointerPressFrame>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PointerTimeline {
    frames: Vec<PointerFrame>,
    duration: f64,
    artwork_by_id: HashMap<String, PointerArtwork>,
    fallback_artwork: Option<PointerArtwork>,
}

struct PressInterval {
    start: f64,
    end: f64,
}

impl PressInterval {
    fn contains(&self, time: f64) -> bool {
        time >= self.start && time <= self.end
    }
}

mod spring {
    use super::SpringConstant;
    pub const GLIDE: SpringConstant = SpringConstant { tension: 470.0, friction: 70.0, inertia: 3.0 };
    pub const INTERCEPT: SpringConstant = SpringConstant { tension: 538.0, friction: 40.0, inertia: 1.0 };
    pub const TRACK: SpringConstant = SpringConstant { tension: 1000.0, friction: 40.0, inertia: 1.0 };
    pub const SETTLE: SpringConstant = SpringConstant { tension: 300.0, friction: 30.0, inertia: 0.3 };
}

/// Everything `PointerTimeline::build` takes besides the capture.
pub struct PointerTimelineOptions<'a> {
    pub recording_size_in_points: (f64, f64),
    pub fallback_artwork: Option<PointerArtwork>,
    /// Seconds of stillness before the cursor fades (with `hide_when_idle`,
    /// None means 3 s).
    pub hide_after_inactivity: Option<f64>,
    pub clips: Option<&'a ClipTimeline>,
    pub cursor: CursorOptions,
    /// The editor-selected style, drawn instead of the recorded artwork.
    pub override_artwork: Option<PointerArtwork>,
}

impl Default for PointerTimelineOptions<'_> {
    fn default() -> Self {
        Self {
            recording_size_in_points: (1000.0, 1000.0),
            fallback_artwork: None,
            hide_after_inactivity: None,
            clips: None,
            cursor: CursorOptions::default(),
            override_artwork: None,
        }
    }
}

impl PointerTimeline {
    pub const STEP_RATE: f64 = 120.0;
    const ANTICIPATION_WINDOW: f64 = 0.5;
    const INTERCEPT_WINDOW: f64 = 0.175;
    const PRESS_ANTICIPATION: f64 = 0.13;
    const ORPHAN_PRESS_HOLD: f64 = 0.15;
    const TILT_SAMPLE_WINDOW: f64 = 0.4;
    const TILT_GAIN: f64 = 0.03;
    const TILT_WEIGHT: f64 = 0.5;
    const REVEAL_LEAD_WINDOW: f64 = 0.25;

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// The interpolated frame at an edited time.
    pub fn frame(&self, time: f64) -> Option<PointerFrame> {
        if self.frames.is_empty() {
            return None;
        }
        if self.frames.len() == 1 || self.duration <= 0.0 {
            return self.frames.first().cloned();
        }
        let position = time.clamp(0.0, self.duration) * Self::STEP_RATE;
        let index = position as usize;
        if index >= self.frames.len() - 1 {
            return self.frames.last().cloned();
        }
        let fraction = position - index as f64;
        let a = &self.frames[index];
        let b = &self.frames[index + 1];
        let lerp = |x: f64, y: f64| x + (y - x) * fraction;
        Some(PointerFrame {
            location: UnitPoint::new(lerp(a.location.x, b.location.x), lerp(a.location.y, b.location.y)),
            artwork_id: if fraction < 0.5 { a.artwork_id.clone() } else { b.artwork_id.clone() },
            magnification: lerp(a.magnification, b.magnification),
            tilt_degrees: lerp(a.tilt_degrees, b.tilt_degrees),
            opacity: lerp(a.opacity, b.opacity),
            blur_radius: lerp(a.blur_radius, b.blur_radius),
            press: if fraction < 0.5 { a.press.clone() } else { b.press.clone() },
        })
    }

    pub fn location(&self, time: f64) -> Option<UnitPoint> {
        self.frame(time).map(|frame| frame.location)
    }

    pub fn artwork(&self, id: Option<&str>) -> Option<&PointerArtwork> {
        match id {
            Some(id) => self.artwork_by_id.get(id).or(self.fallback_artwork.as_ref()),
            None => self.fallback_artwork.as_ref(),
        }
    }

    pub fn build(capture: &PointerCaptureFile, duration: f64, options: PointerTimelineOptions<'_>) -> Self {
        if !duration.is_finite() || duration <= 0.0 {
            return Self::empty();
        }
        let timeline_duration = options.clips.map(ClipTimeline::duration).unwrap_or(duration);
        if !timeline_duration.is_finite() || timeline_duration <= 0.0 {
            return Self::empty();
        }
        let safe_size = sanitized_size(options.recording_size_in_points);
        let stream = sanitize(capture, &PointerSanitizeOptions::new(safe_size));
        let source_samples: Vec<PointerStreamEvent> =
            stream.samples.into_iter().filter(|s| s.time.is_finite() && s.x.is_finite() && s.y.is_finite()).collect();
        let samples = timeline_samples(source_samples, options.clips);
        let Some(first_sample) = samples.first().cloned() else { return Self::empty() };

        let press_samples: Vec<&PointerStreamEvent> = samples.iter().filter(|s| s.kind == StreamKind::Press).collect();
        // Stationary keep-alive samples must not prevent the idle cursor from fading.
        let mut travel_samples: Vec<&PointerStreamEvent> = Vec::new();
        for sample in samples.iter().filter(|s| matches!(s.kind, StreamKind::Travel | StreamKind::Drag)) {
            if let Some(previous) = travel_samples.last() {
                if ((sample.x - previous.x) * safe_size.0).hypot((sample.y - previous.y) * safe_size.1) < 1.0 {
                    continue;
                }
            }
            travel_samples.push(sample);
        }
        let intervals = press_intervals(&samples, timeline_duration);

        let mut artwork_by_id: HashMap<String, PointerArtwork> = HashMap::new();
        for artwork in stream.artwork {
            artwork_by_id.entry(artwork.artwork_id.clone()).or_insert(artwork);
        }
        if let Some(override_artwork) = &options.override_artwork {
            artwork_by_id.insert(override_artwork.artwork_id.clone(), override_artwork.clone());
        }
        let cursor = &options.cursor;
        let idle_delay = if cursor.hide_when_idle {
            Some(options.hide_after_inactivity.unwrap_or(3.0))
        } else {
            options.hide_after_inactivity
        };
        let frame_count = ((timeline_duration * Self::STEP_RATE).ceil() as usize + 1).max(2);
        let dt = 1.0 / Self::STEP_RATE;
        let mut x_spring = DampedSpring::new(first_sample.x);
        let mut y_spring = DampedSpring::new(first_sample.y);
        let mut magnification_spring = DampedSpring::new(1.0);
        let mut opacity_spring = DampedSpring::new(1.0);
        let mut blur_spring = DampedSpring::new(0.0);

        let mut sample_index: Option<usize> = None;
        let mut travel_index: Option<usize> = None;
        let mut latest_press_index: Option<usize> = None;
        let mut last_interaction_time = 0.0;
        let mut press_interval_index = 0;
        let mut current_artwork_id = first_sample.artwork_id.clone();
        let mut frames: Vec<PointerFrame> = Vec::with_capacity(frame_count);

        for frame_index in 0..frame_count {
            let time = (frame_index as f64 * dt).min(timeline_duration);

            while samples.get(sample_index.map_or(0, |i| i + 1)).is_some_and(|s| s.time <= time) {
                let next = sample_index.map_or(0, |i| i + 1);
                sample_index = Some(next);
                let sample = &samples[next];
                current_artwork_id = sample.artwork_id.clone();
                if matches!(sample.kind, StreamKind::Press | StreamKind::Release) {
                    last_interaction_time = sample.time;
                }
            }
            while travel_samples.get(travel_index.map_or(0, |i| i + 1)).is_some_and(|s| s.time <= time) {
                travel_index = Some(travel_index.map_or(0, |i| i + 1));
            }
            while press_samples.get(latest_press_index.map_or(0, |i| i + 1)).is_some_and(|s| s.time <= time) {
                latest_press_index = Some(latest_press_index.map_or(0, |i| i + 1));
            }
            while intervals.get(press_interval_index).is_some_and(|i| i.end < time) {
                press_interval_index += 1;
            }

            let latest = sample_index.map(|i| &samples[i]).unwrap_or(&first_sample);
            let mut target = latest.location();
            let mut approaching_press = false;
            let upcoming = latest_press_index.map_or(0, |i| i + 1);
            if let Some(press) = press_samples.get(upcoming) {
                let remaining = press.time - time;
                if (0.0..=Self::ANTICIPATION_WINDOW).contains(&remaining) {
                    target = press.location();
                    approaching_press = remaining <= Self::INTERCEPT_WINDOW;
                }
            }

            let is_pressed = intervals.get(press_interval_index).is_some_and(|i| i.contains(time));
            // A drag sample is authoritative even if a press was lost; a
            // release becomes the latest sample and returns to the glide.
            let is_dragging = latest.kind == StreamKind::Drag;
            let motion = if is_dragging {
                spring::TRACK
            } else if approaching_press {
                spring::INTERCEPT
            } else {
                spring::GLIDE
            };
            x_spring.step(target.x, motion, dt);
            y_spring.step(target.y, motion, dt);

            let mut is_hidden = false;
            if let (Some(idle_delay), Some(travel_index)) = (idle_delay, travel_index) {
                if idle_delay > 0.0 && !is_pressed {
                    let previous_travel = travel_samples[travel_index];
                    let will_move_soon = travel_samples
                        .get(travel_index + 1)
                        .is_some_and(|next| next.time - time <= Self::REVEAL_LEAD_WINDOW);
                    is_hidden = time - previous_travel.time.max(last_interaction_time) > idle_delay && !will_move_soon;
                }
            }

            let target_magnification =
                (if is_pressed && cursor.press_effect { 0.8 } else { 1.0 }) * (if is_hidden { 0.8 } else { 1.0 });
            magnification_spring.step(target_magnification, spring::SETTLE, dt);
            opacity_spring.step(if is_hidden { 0.0 } else { 1.0 }, spring::SETTLE, dt);
            blur_spring.step(if is_hidden { 5.0 } else { 0.0 }, spring::SETTLE, dt);

            let press = latest_press_index.map(|i| press_samples[i]).and_then(|event| {
                let elapsed = time - event.time;
                (elapsed <= PRESS_EFFECT_DURATION).then(|| PointerPressFrame {
                    location: event.location(),
                    progress: (elapsed / PRESS_EFFECT_DURATION).clamp(0.0, 1.0),
                    impact_enabled: cursor.press_effect,
                    ripple_enabled: cursor.ripple_effect,
                })
            });

            frames.push(PointerFrame {
                location: if cursor.smooth_motion {
                    UnitPoint::new(x_spring.position, y_spring.position)
                } else {
                    latest.location()
                },
                artwork_id: options
                    .override_artwork
                    .as_ref()
                    .map(|a| a.artwork_id.clone())
                    .or_else(|| current_artwork_id.clone()),
                magnification: magnification_spring.position,
                tilt_degrees: 0.0,
                opacity: if cursor.is_visible { opacity_spring.position.clamp(0.0, 1.0) } else { 0.0 },
                blur_radius: blur_spring.position.max(0.0),
                press: if cursor.is_visible { press } else { None },
            });
        }

        let tilt_offset = ((Self::TILT_SAMPLE_WINDOW * Self::STEP_RATE).round() as usize).max(1);
        let recording_width = safe_size.0.max(1.0);
        for index in 0..frames.len() {
            let previous = frames[index.saturating_sub(tilt_offset)].location;
            let current = frames[index].location;
            let delta_in_points = (current.x - previous.x) * recording_width;
            frames[index].tilt_degrees = if cursor.smooth_motion {
                (delta_in_points * Self::TILT_GAIN * Self::TILT_WEIGHT).clamp(-20.0, 20.0)
            } else {
                0.0
            };
        }

        Self { frames, duration: timeline_duration, artwork_by_id, fallback_artwork: options.fallback_artwork }
    }
}

/// Maps retained source events onto edited time before spring integration.
/// Removed clicks can no longer attract the cursor through look-ahead, and
/// a seed at every clip boundary lets the spring bridge cuts smoothly.
fn timeline_samples(source: Vec<PointerStreamEvent>, clips: Option<&ClipTimeline>) -> Vec<PointerStreamEvent> {
    let Some(clips) = clips else { return source };
    if source.is_empty() || clips.segments.is_empty() {
        return Vec::new();
    }
    // (event, boundary seed, order)
    let mut ranked: Vec<(PointerStreamEvent, bool, usize)> = Vec::with_capacity(source.len() + clips.segments.len());
    for (index, sample) in source.iter().enumerate() {
        if let Some(editor_time) = editor_time_for_source_event(sample.time, clips) {
            let mut mapped = sample.clone();
            mapped.time = editor_time;
            ranked.push((mapped, false, index));
        }
    }
    let mut editor_start = 0.0;
    for (index, segment) in clips.segments.iter().enumerate() {
        if let Some(mut seed) = source_sample_at(segment.source_start, &source) {
            seed.time = editor_start;
            seed.kind = StreamKind::Travel;
            seed.button = None;
            ranked.push((seed, true, index));
        }
        editor_start += segment.editor_duration();
    }
    ranked.sort_by(|a, b| {
        a.0.time
            .partial_cmp(&b.0.time)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.1.cmp(&a.1)) // boundary seeds first
            .then(a.2.cmp(&b.2))
    });
    ranked.into_iter().map(|(event, _, _)| event).collect()
}

/// Events use half-open source ranges, so an event at an outgoing clip's
/// exact end cannot leak onto the first frame of the next clip.
fn editor_time_for_source_event(source_time: f64, clips: &ClipTimeline) -> Option<f64> {
    let mut editor_start = 0.0;
    for segment in &clips.segments {
        if source_time >= segment.source_start && source_time < segment.source_end {
            return Some(editor_start + (source_time - segment.source_start) / segment.speed.max(1.0));
        }
        editor_start += segment.editor_duration();
    }
    None
}

fn source_sample_at(time: f64, samples: &[PointerStreamEvent]) -> Option<PointerStreamEvent> {
    let first = samples.first()?;
    let index = samples.partition_point(|s| s.time <= time);
    Some(if index > 0 { samples[index - 1].clone() } else { first.clone() })
}

fn press_intervals(samples: &[PointerStreamEvent], duration: f64) -> Vec<PressInterval> {
    let mut result: Vec<PressInterval> = Vec::new();
    let mut unmatched: HashMap<i64, Vec<&PointerStreamEvent>> = HashMap::new();
    let mut unmatched_order: Vec<i64> = Vec::new();
    for sample in samples.iter().filter(|s| matches!(s.kind, StreamKind::Press | StreamKind::Release)) {
        let button = sample.button.unwrap_or(0);
        if sample.kind == StreamKind::Press {
            if !unmatched.contains_key(&button) {
                unmatched_order.push(button);
            }
            unmatched.entry(button).or_default().push(sample);
        } else if let Some(pending) = unmatched.get_mut(&button).filter(|p| !p.is_empty()) {
            let press = pending.remove(0);
            result.push(PressInterval {
                start: (press.time - PointerTimeline::PRESS_ANTICIPATION).max(0.0),
                end: sample.time.max(press.time).min(duration),
            });
        }
    }
    for button in unmatched_order {
        for press in unmatched.get(&button).into_iter().flatten() {
            result.push(PressInterval {
                start: (press.time - PointerTimeline::PRESS_ANTICIPATION).max(0.0),
                end: (press.time + PointerTimeline::ORPHAN_PRESS_HOLD).min(duration),
            });
        }
    }
    result.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.end.partial_cmp(&b.end).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut merged: Vec<PressInterval> = Vec::new();
    for interval in result {
        match merged.last_mut() {
            Some(last) if interval.start <= last.end => last.end = last.end.max(interval.end),
            _ => merged.push(interval),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::ClipSegment;

    fn travel(time: f64, x: f64, y: f64) -> PointerTravelSample {
        PointerTravelSample { time, x, y, kind: PointerTravelKind::Move, artwork_id: None }
    }

    fn press(time: f64, x: f64, y: f64, phase: PressPhase) -> PointerPressEvent {
        PointerPressEvent { time, x, y, button: 0, phase, artwork_id: None }
    }

    #[test]
    fn sanitizing_orders_clamps_and_thins() {
        let capture = PointerCaptureFile {
            travel: vec![
                travel(1.0, 0.5, 0.5),
                travel(0.0, 0.1, 0.1),
                travel(0.004, 0.1001, 0.1), // too soon and too close: thinned
                travel(0.5, 1.2, 0.5),      // off-frame: dropped
                travel(0.6, 1.0005, 0.5),   // boundary tolerance: clamped
                travel(f64::NAN, 0.2, 0.2),
            ],
            presses: vec![press(0.3, 0.2, 0.2, PressPhase::Up), press(0.2, 0.2, 0.2, PressPhase::Down)],
            ..Default::default()
        };
        let stream = sanitize(&capture, &PointerSanitizeOptions::new((1000.0, 1000.0)));
        let times: Vec<f64> = stream.samples.iter().map(|s| s.time).collect();
        assert_eq!(times, vec![0.0, 0.2, 0.3, 0.6, 1.0]);
        assert_eq!(stream.samples[3].x, 1.0);
        assert_eq!(stream.samples[1].kind, StreamKind::Press);
        assert_eq!(stream.samples[2].kind, StreamKind::Release);
        assert!(stream.sanitized_capture.is_sanitized);
        // Sanitizing again is the identity.
        let again = sanitize(&stream.sanitized_capture, &PointerSanitizeOptions::new((1000.0, 1000.0)));
        assert_eq!(again.samples, stream.samples);
    }

    #[test]
    fn isolated_spikes_are_dropped() {
        let capture = PointerCaptureFile {
            travel: vec![travel(0.0, 0.1, 0.1), travel(0.1, 0.9, 0.9), travel(0.2, 0.11, 0.1), travel(0.3, 0.12, 0.1)],
            ..Default::default()
        };
        let stream = sanitize(&capture, &PointerSanitizeOptions::new((1000.0, 1000.0)));
        assert!(stream.samples.iter().all(|s| s.x < 0.5), "{:?}", stream.samples);
        assert_eq!(stream.samples.len(), 3);
    }

    #[test]
    fn artwork_without_pixels_is_invalid_but_styled_ids_resolve() {
        let capture = PointerCaptureFile {
            travel: vec![travel(0.0, 0.1, 0.1), travel(1.0, 0.2, 0.2)],
            artwork: vec![PointerArtwork {
                artwork_id: "a".into(),
                image_data: Vec::new(),
                anchor_point: ArtworkPoint { x: 1.0, y: 1.0 },
                reference_size: ArtworkSize { width: 16.0, height: 16.0 },
            }],
            ..Default::default()
        };
        let stream = sanitize(&capture, &PointerSanitizeOptions::new((1000.0, 1000.0)));
        assert!(stream.artwork.is_empty());
        let arrow = styled_artwork(CursorAppearance::Arrow).unwrap();
        assert_eq!(arrow.artwork_id, "bettershot-cursor-macOS");
        assert_eq!(arrow.styled_appearance(), Some(CursorAppearance::Arrow));
        assert!(arrow.is_styled());
        assert_eq!(arrow.normalized_anchor(), UnitPoint::new(5.0 / 32.0, 4.0 / 40.0));
        assert_eq!(arrow.aspect_ratio(), 0.8);
        assert_eq!(arrow.intrinsic_scale(), 2.5);
        assert!(styled_artwork(CursorAppearance::Recorded).is_none());
    }

    #[test]
    fn timeline_glides_toward_samples_and_marks_presses() {
        let capture = PointerCaptureFile {
            travel: vec![travel(0.0, 0.1, 0.1), travel(1.0, 0.9, 0.9)],
            presses: vec![press(1.5, 0.9, 0.9, PressPhase::Down), press(1.6, 0.9, 0.9, PressPhase::Up)],
            ..Default::default()
        };
        let timeline = PointerTimeline::build(&capture, 3.0, PointerTimelineOptions::default());
        assert!(!timeline.is_empty());
        assert_eq!(timeline.duration(), 3.0);
        let start = timeline.frame(0.0).unwrap();
        assert!((start.location.x - 0.1).abs() < 0.01);
        // Shortly after the second sample the spring is on its way; by 3 s it has arrived.
        let moving = timeline.frame(1.1).unwrap();
        assert!(moving.location.x > 0.1 && moving.location.x < 0.9, "{moving:?}");
        let settled = timeline.frame(2.9).unwrap();
        assert!((settled.location.x - 0.9).abs() < 0.01, "{settled:?}");
        // The press pulse lasts 0.4 s and shrinks the cursor while held.
        let pressed = timeline.frame(1.55).unwrap();
        let pulse = pressed.press.as_ref().expect("press frame");
        assert!((pulse.location.x - 0.9).abs() < 1e-9 && pulse.progress > 0.1 && pulse.progress < 0.2);
        assert!(pressed.magnification < 0.95, "{pressed:?}");
        assert!(timeline.frame(2.5).unwrap().press.is_none());
        // Smooth motion off follows samples exactly, with no tilt.
        let raw = PointerTimeline::build(
            &capture,
            3.0,
            PointerTimelineOptions {
                cursor: CursorOptions { smooth_motion: false, ..Default::default() },
                ..Default::default()
            },
        );
        let exact = raw.frame(1.0).unwrap();
        assert_eq!((exact.location.x, exact.tilt_degrees), (0.9, 0.0));
        // Hidden cursors keep no press frames and zero opacity.
        let hidden = PointerTimeline::build(
            &capture,
            3.0,
            PointerTimelineOptions {
                cursor: CursorOptions { is_visible: false, ..Default::default() },
                ..Default::default()
            },
        );
        let frame = hidden.frame(1.55).unwrap();
        assert_eq!(frame.opacity, 0.0);
        assert!(frame.press.is_none());
    }

    #[test]
    fn idle_cursor_fades_and_returns_before_moving() {
        let capture = PointerCaptureFile {
            travel: vec![travel(0.0, 0.5, 0.5), travel(0.1, 0.52, 0.5), travel(5.0, 0.8, 0.5)],
            ..Default::default()
        };
        let options = PointerTimelineOptions {
            cursor: CursorOptions { hide_when_idle: true, ..Default::default() },
            ..Default::default()
        };
        let timeline = PointerTimeline::build(&capture, 6.0, options);
        assert!(timeline.frame(2.0).unwrap().opacity > 0.9, "still visible within 3 s");
        assert!(timeline.frame(4.5).unwrap().opacity < 0.2, "faded after 3 s idle");
        assert!(timeline.frame(5.4).unwrap().opacity > 0.8, "back once it moves");
        let without = PointerTimeline::build(&capture, 6.0, PointerTimelineOptions::default());
        assert!(without.frame(4.5).unwrap().opacity > 0.99);
    }

    #[test]
    fn clips_remap_events_and_seed_boundaries() {
        let capture = PointerCaptureFile {
            travel: vec![travel(0.0, 0.1, 0.1), travel(2.0, 0.5, 0.5), travel(4.0, 0.9, 0.9)],
            presses: vec![press(2.5, 0.5, 0.5, PressPhase::Down), press(2.6, 0.5, 0.5, PressPhase::Up)],
            ..Default::default()
        };
        // Keep 0–1 s and 4–5 s: the click in the middle is removed.
        let clips = ClipTimeline::new(vec![ClipSegment::new(0.0, 1.0), ClipSegment::new(4.0, 5.0)]);
        let options = PointerTimelineOptions { clips: Some(&clips), ..Default::default() };
        let timeline = PointerTimeline::build(&capture, 5.0, options);
        assert_eq!(timeline.duration(), 2.0);
        assert!(timeline.frame(0.5).unwrap().press.is_none());
        assert!(timeline.frame(1.2).unwrap().press.is_none(), "the removed click never shows");
        // After the cut the seed from the second clip's start pulls the cursor to 0.9.
        let late = timeline.frame(1.9).unwrap();
        assert!(late.location.x > 0.7, "{late:?}");
        // Speed shortens the edited timeline; events land at scaled times.
        let fast = ClipTimeline::new(vec![ClipSegment::with_speed(0.0, 4.0, 2.0)]);
        let timeline =
            PointerTimeline::build(&capture, 5.0, PointerTimelineOptions { clips: Some(&fast), ..Default::default() });
        assert_eq!(timeline.duration(), 2.0);
        assert!(timeline.frame(1.3).unwrap().press.is_some(), "the click at 2.5 s plays at 1.25 s");
    }

    #[test]
    fn press_effect_geometry_matches_the_macos_curve() {
        let start = press_effect_geometry(0.0, 1080.0, 1.0);
        assert!((start.impact_radius - 21.0 * 0.38).abs() < 1e-9);
        assert!((start.impact_opacity - 0.38).abs() < 1e-9);
        assert_eq!(start.ripple_opacity, 0.0);
        let end = press_effect_geometry(1.0, 1080.0, 1.0);
        assert_eq!(end.impact_opacity, 0.0);
        assert!((end.ripple_radius - 21.0 * 1.55).abs() < 1e-9);
        assert!(end.ripple_opacity.abs() < 1e-9);
        assert_eq!(end.ripple_line_width, (21.0_f64 * 0.07).max(1.0));
        let scaled = press_effect_geometry(0.0, 540.0, 2.0);
        assert!((scaled.impact_radius - start.impact_radius).abs() < 1e-9, "scale and height cancel");
    }

    #[test]
    fn orphan_presses_hold_briefly_and_intervals_merge() {
        let samples = vec![
            PointerStreamEvent {
                time: 1.0,
                x: 0.5,
                y: 0.5,
                kind: StreamKind::Press,
                button: Some(0),
                artwork_id: None,
            },
            PointerStreamEvent {
                time: 1.05,
                x: 0.5,
                y: 0.5,
                kind: StreamKind::Press,
                button: Some(1),
                artwork_id: None,
            },
            PointerStreamEvent {
                time: 1.3,
                x: 0.5,
                y: 0.5,
                kind: StreamKind::Release,
                button: Some(1),
                artwork_id: None,
            },
        ];
        let intervals = press_intervals(&samples, 10.0);
        assert_eq!(intervals.len(), 1, "overlapping intervals merge");
        assert!((intervals[0].start - 0.87).abs() < 1e-9);
        assert!((intervals[0].end - 1.3).abs() < 1e-9);
    }
}
