// SPDX-License-Identifier: AGPL-3.0-only
// Adapts Cap zoom rendering, Copyright (c) 2023-present Cap Software, Inc.
// Swift adaptation Copyright (c) 2026 Kartik Labhshetwar.
// Rust port for the Linux app follows that adaptation.
// See Resources/Licenses/Cap.txt for the full license.
//
//! Port of `RecordingViewportTimeline.swift` and `RecordingMotionSpring.swift`:
//! deterministic virtual-camera planning. Zoom cues are editable project
//! data; this resolves them against the pointer capture, constrains the
//! viewport to the source, and integrates damped springs at a fixed rate
//! along the *edited* timeline, so cuts never interrupt a zoom and per-clip
//! speed never changes how fast the camera moves.

use super::clips::ClipTimeline;
use super::edit::UnitRect;
use super::manifest::{PointerCaptureFile, PressPhase};
use crate::capture_record::uppercase_uuid;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ZoomAnchorMode {
    /// Follow pointer activity regions, re-aiming when the pointer leaves one.
    #[default]
    PointerAnchor,
    /// Hold on stable regions of pointer activity.
    SmartAnchor,
    /// Frame an explicit normalized point.
    PinnedAnchor,
}

impl ZoomAnchorMode {
    pub const ALL: [Self; 3] = [Self::PointerAnchor, Self::SmartAnchor, Self::PinnedAnchor];

    pub fn title(self) -> &'static str {
        match self {
            Self::PointerAnchor => "Auto",
            Self::SmartAnchor => "Smart",
            Self::PinnedAnchor => "Pinned",
        }
    }
}

impl<'de> Deserialize<'de> for ZoomAnchorMode {
    /// Unknown and legacy modes keep the pointer-follow behavior.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "smartAnchor" => Self::SmartAnchor,
            "pinnedAnchor" => Self::PinnedAnchor,
            _ => Self::PointerAnchor,
        })
    }
}

/// A `CGPoint`, which Foundation encodes as `[x, y]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitPoint {
    pub x: f64,
    pub y: f64,
}

impl UnitPoint {
    pub const CENTER: Self = Self { x: 0.5, y: 0.5 };

    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn normalized(self) -> Self {
        Self { x: unit(self.x), y: unit(self.y) }
    }
}

impl Serialize for UnitPoint {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        [self.x, self.y].serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UnitPoint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Array([f64; 2]),
            Object { x: f64, y: f64 },
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Array([x, y]) => Self { x, y },
            Raw::Object { x, y } => Self { x, y },
        })
    }
}

fn unit(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.5
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoomCue {
    #[serde(with = "uppercase_uuid")]
    pub id: Uuid,
    pub start: f64,
    pub end: f64,
    /// Magnification while fully active (1 = no zoom).
    pub zoom: f64,
    pub anchor_mode: ZoomAnchorMode,
    pub pinned_point: UnitPoint,
    /// How strongly automatic targets retain their unzoomed screen position.
    pub bounds_bias: f64,
    pub is_enabled: bool,
    pub is_implicit: bool,
    pub skips_easing: bool,
}

impl ZoomCue {
    /// Shortest cue the timeline creates or leaves behind after a resize.
    pub const MINIMUM_DURATION: f64 = 0.5;

    pub fn new(start: f64, end: f64) -> Self {
        Self {
            id: Uuid::new_v4(),
            start,
            end,
            zoom: 1.5,
            anchor_mode: ZoomAnchorMode::PointerAnchor,
            pinned_point: UnitPoint::CENTER,
            bounds_bias: 0.25,
            is_enabled: true,
            is_implicit: false,
            skips_easing: false,
        }
    }

    pub fn with_zoom(mut self, zoom: f64) -> Self {
        self.zoom = zoom;
        self
    }

    pub fn pinned(mut self, point: UnitPoint) -> Self {
        self.anchor_mode = ZoomAnchorMode::PinnedAnchor;
        self.pinned_point = point.normalized();
        self
    }

    pub fn with_anchor_mode(mut self, mode: ZoomAnchorMode) -> Self {
        self.anchor_mode = mode;
        self
    }

    pub fn instant(mut self) -> Self {
        self.skips_easing = true;
        self
    }

    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }
}

impl<'de> Deserialize<'de> for ZoomCue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            id: Option<String>,
            start: f64,
            end: f64,
            zoom: Option<f64>,
            anchor_mode: Option<ZoomAnchorMode>,
            pinned_point: Option<UnitPoint>,
            bounds_bias: Option<f64>,
            is_enabled: Option<bool>,
            is_implicit: Option<bool>,
            skips_easing: Option<bool>,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(Self {
            id: raw.id.and_then(|id| Uuid::parse_str(&id).ok()).unwrap_or_else(Uuid::new_v4),
            start: raw.start,
            end: raw.end,
            zoom: raw.zoom.unwrap_or(2.0),
            anchor_mode: raw.anchor_mode.unwrap_or_default(),
            pinned_point: raw.pinned_point.unwrap_or(UnitPoint::CENTER).normalized(),
            bounds_bias: unit(raw.bounds_bias.unwrap_or(0.25)),
            is_enabled: raw.is_enabled.unwrap_or(true),
            is_implicit: raw.is_implicit.unwrap_or(false),
            skips_easing: raw.skips_easing.unwrap_or(false),
        })
    }
}

/// One sampled state of the virtual camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportFrame {
    pub magnification: f64,
    /// Normalized content point at the viewport center.
    pub anchor: UnitPoint,
}

impl ViewportFrame {
    pub const IDENTITY: Self = Self { magnification: 1.0, anchor: UnitPoint::CENTER };

    pub fn cropped(self, crop: &UnitRect) -> Self {
        if crop.is_unit() {
            return self;
        }
        Self {
            magnification: self.magnification,
            anchor: UnitPoint::new(
                UnitRect::anchor(self.anchor.x, crop.x, crop.width, self.magnification),
                UnitRect::anchor(self.anchor.y, crop.y, crop.height, self.magnification),
            ),
        }
    }
}

// MARK: - Springs

#[derive(Debug, Clone, Copy)]
pub struct SpringConstant {
    pub tension: f64,
    pub friction: f64,
    pub inertia: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct DampedSpring {
    pub position: f64,
    pub velocity: f64,
}

impl DampedSpring {
    pub fn new(position: f64) -> Self {
        Self { position, velocity: 0.0 }
    }

    pub fn snap(&mut self, value: f64) {
        self.position = value;
        self.velocity = 0.0;
    }

    /// Semi-implicit Euler step toward `target`.
    pub fn step(&mut self, target: f64, constant: SpringConstant, dt: f64) {
        if constant.inertia <= 0.0 {
            self.position = target;
            self.velocity = 0.0;
            return;
        }
        let acceleration =
            (constant.tension * (target - self.position) - constant.friction * self.velocity) / constant.inertia;
        self.velocity += acceleration * dt;
        self.position += self.velocity * dt;
    }
}

// MARK: - Auto-generation from recorded input

/// Builds editable pointer cues around press events; cues connected by the
/// allowed gap merge into one continuous zoom.
pub fn synthesize_cues(capture: &PointerCaptureFile, duration: f64) -> Vec<ZoomCue> {
    const PRE_ROLL: f64 = 0.3;
    const POST_ROLL: f64 = 2.5;
    const JOIN_TOLERANCE: f64 = 2.5;
    const TAIL_EXCLUSION: f64 = 1.0;
    const TRAILING_GUARD: f64 = 0.8;
    const EARLIEST_START: f64 = 0.001;
    if !duration.is_finite() || duration <= 0.0 {
        return Vec::new();
    }
    let latest = duration - TAIL_EXCLUSION;
    let mut presses: Vec<_> = capture
        .presses
        .iter()
        .filter(|p| {
            p.phase == PressPhase::Down
                && p.time.is_finite()
                && p.time < latest
                && (0.0..=1.0).contains(&p.x)
                && (0.0..=1.0).contains(&p.y)
        })
        .collect();
    presses.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged: Vec<ZoomCue> = Vec::new();
    for press in presses {
        let start = (press.time - PRE_ROLL).max(EARLIEST_START);
        let end = (press.time + POST_ROLL).min(duration - TRAILING_GUARD);
        if end <= start {
            continue;
        }
        let mut cue = ZoomCue::new(start, end).with_zoom(2.0);
        cue.pinned_point = UnitPoint::new(press.x, press.y).normalized();
        match merged.last_mut() {
            Some(previous) if cue.start <= previous.end + JOIN_TOLERANCE => previous.end = previous.end.max(cue.end),
            _ => merged.push(cue),
        }
    }
    merged
}

// MARK: - Precomputed viewport timeline

#[derive(Debug, Clone, PartialEq)]
pub struct ViewportTimeline {
    frames: Vec<ViewportFrame>,
    duration: f64,
}

#[derive(Clone, Copy)]
struct PointerSample {
    time: f64,
    point: UnitPoint,
    press: bool,
}

struct ActivityTarget {
    activation_time: f64,
    point: UnitPoint,
}

struct ActivityGroup {
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
    first_time: f64,
    first_press_time: Option<f64>,
}

impl ActivityGroup {
    fn new(sample: PointerSample) -> Self {
        Self {
            min_x: sample.point.x,
            max_x: sample.point.x,
            min_y: sample.point.y,
            max_y: sample.point.y,
            first_time: sample.time,
            first_press_time: sample.press.then_some(sample.time),
        }
    }

    fn center(&self) -> UnitPoint {
        UnitPoint::new((self.min_x + self.max_x) / 2.0, (self.min_y + self.max_y) / 2.0)
    }

    fn can_include(&self, sample: PointerSample, horizontal: f64, vertical: f64) -> bool {
        self.max_x.max(sample.point.x) - self.min_x.min(sample.point.x) <= horizontal
            && self.max_y.max(sample.point.y) - self.min_y.min(sample.point.y) <= vertical
    }

    fn include(&mut self, sample: PointerSample) {
        self.min_x = self.min_x.min(sample.point.x);
        self.max_x = self.max_x.max(sample.point.x);
        self.min_y = self.min_y.min(sample.point.y);
        self.max_y = self.max_y.max(sample.point.y);
        if self.first_press_time.is_none() && sample.press {
            self.first_press_time = Some(sample.time);
        }
    }
}

impl ViewportTimeline {
    /// Fixed integration cadence; playback only interpolates this timeline.
    pub const STEP_RATE: f64 = 120.0;
    const MOTION: SpringConstant = SpringConstant { tension: 200.0, friction: 40.0, inertia: 2.25 };
    const INSTANT_SNAP_WINDOW: f64 = 0.1;
    const PRE_AIM_MAGNIFICATION: f64 = 1.0005;

    pub fn identity() -> Self {
        Self { frames: vec![ViewportFrame::IDENTITY], duration: 0.0 }
    }

    /// A timeline sampled at `STEP_RATE` by another planner (the reframe track).
    pub fn from_frames(frames: Vec<ViewportFrame>, duration: f64) -> Self {
        if frames.is_empty() {
            return Self::identity();
        }
        Self { frames, duration }
    }

    pub fn cropped(&self, crop: &UnitRect) -> Self {
        if crop.is_unit() {
            return self.clone();
        }
        Self { frames: self.frames.iter().map(|f| f.cropped(crop)).collect(), duration: self.duration }
    }

    /// The camera at an editor (edited-timeline) time.
    pub fn frame(&self, time: f64) -> ViewportFrame {
        if self.frames.len() < 2 || self.duration <= 0.0 {
            return self.frames.first().copied().unwrap_or(ViewportFrame::IDENTITY);
        }
        let position = time.clamp(0.0, self.duration) * Self::STEP_RATE;
        let index = position as usize;
        if index >= self.frames.len() - 1 {
            return self.frames[self.frames.len() - 1];
        }
        let fraction = position - index as f64;
        let a = self.frames[index];
        let b = self.frames[index + 1];
        ViewportFrame {
            magnification: a.magnification + (b.magnification - a.magnification) * fraction,
            anchor: UnitPoint::new(
                a.anchor.x + (b.anchor.x - a.anchor.x) * fraction,
                a.anchor.y + (b.anchor.y - a.anchor.y) * fraction,
            ),
        }
    }

    /// Builds the camera's motion along the edited timeline: a cut never
    /// interrupts a zoom, and clip speed never changes the camera's pace.
    pub fn build(cues: &[ZoomCue], capture: &PointerCaptureFile, clips: &ClipTimeline) -> Self {
        let duration = clips.duration();
        if !duration.is_finite() || duration <= 0.0 {
            return Self::identity();
        }
        let samples: Vec<PointerSample> =
            merged_pointer_samples(capture).into_iter().filter(|s| is_retained(s.time, clips)).collect();
        let mut targets_by_cue: HashMap<Uuid, Vec<ActivityTarget>> = HashMap::new();
        for cue in cues.iter().filter(|c| c.anchor_mode != ZoomAnchorMode::PinnedAnchor) {
            targets_by_cue.entry(cue.id).or_insert_with(|| activity_targets(cue, &samples));
        }
        let instant: Vec<&ZoomCue> = cues.iter().filter(|c| c.is_enabled && c.skips_easing).collect();
        let frame_count = ((duration * Self::STEP_RATE).ceil() as usize + 1).max(2);
        let dt = 1.0 / Self::STEP_RATE;

        let mut magnification = DampedSpring::new(1.0);
        let mut center_x = DampedSpring::new(0.5);
        let mut center_y = DampedSpring::new(0.5);
        let mut held = UnitPoint::CENTER;
        let mut frames = Vec::with_capacity(frame_count);

        for frame_index in 0..frame_count {
            let editor_time = (frame_index as f64 * dt).min(duration);
            let time = clips.source_time(editor_time);
            let active = active_cue(time, cues);
            let target_magnification = active.map(|c| c.zoom).unwrap_or(1.0).max(1.0);
            if let Some(active) = active.filter(|_| target_magnification > 1.0) {
                let point =
                    anchor_point(active, time, targets_by_cue.get(&active.id).map(Vec::as_slice).unwrap_or(&[]));
                let anchor = bounded_anchor(point, target_magnification, active.anchor_mode, active.bounds_bias);
                held = travel_center(anchor, target_magnification);
            }
            let snaps = instant
                .iter()
                .any(|c| time >= c.start - Self::INSTANT_SNAP_WINDOW && time <= c.end + Self::INSTANT_SNAP_WINDOW);
            if snaps {
                magnification.snap(target_magnification);
                center_x.snap(held.x);
                center_y.snap(held.y);
            } else {
                if magnification.position <= Self::PRE_AIM_MAGNIFICATION {
                    center_x.snap(held.x);
                    center_y.snap(held.y);
                }
                if frame_index > 0 {
                    magnification.step(target_magnification, Self::MOTION, dt);
                    center_x.step(held.x, Self::MOTION, dt);
                    center_y.step(held.y, Self::MOTION, dt);
                }
            }
            if magnification.position < 1.0 {
                magnification.snap(1.0);
            }
            let m = magnification.position;
            let half = 1.0 / (2.0 * m);
            let center = UnitPoint::new(center_x.position, center_y.position).normalized();
            frames.push(ViewportFrame {
                magnification: m,
                anchor: UnitPoint::new(half + center.x * (1.0 - 2.0 * half), half + center.y * (1.0 - 2.0 * half)),
            });
        }
        Self { frames, duration }
    }
}

/// Converts a viewport center to Cap's travel-space center.
fn travel_center(anchor: UnitPoint, magnification: f64) -> UnitPoint {
    let half = 1.0 / (2.0 * magnification);
    let travel = 1.0 - 2.0 * half;
    if travel <= 0.000_001 {
        return UnitPoint::CENTER;
    }
    UnitPoint::new((anchor.x - half) / travel, (anchor.y - half) / travel).normalized()
}

/// User-authored intent wins; within a class the most recently started cue wins.
fn active_cue(time: f64, cues: &[ZoomCue]) -> Option<&ZoomCue> {
    let mut selected: Option<&ZoomCue> = None;
    let mut selected_precedence = i32::MIN;
    for cue in cues.iter().filter(|c| c.is_enabled && time >= c.start && time <= c.end) {
        let precedence = cue_priority(cue);
        let wins = match selected {
            None => true,
            Some(current) => {
                precedence > selected_precedence || (precedence == selected_precedence && cue.start >= current.start)
            }
        };
        if wins {
            selected = Some(cue);
            selected_precedence = precedence;
        }
    }
    selected
}

fn cue_priority(cue: &ZoomCue) -> i32 {
    if cue.is_implicit {
        return 0;
    }
    match cue.anchor_mode {
        ZoomAnchorMode::PointerAnchor => 1,
        ZoomAnchorMode::SmartAnchor => 2,
        ZoomAnchorMode::PinnedAnchor => 3,
    }
}

fn anchor_point(cue: &ZoomCue, time: f64, targets: &[ActivityTarget]) -> UnitPoint {
    match cue.anchor_mode {
        ZoomAnchorMode::PinnedAnchor => cue.pinned_point.normalized(),
        ZoomAnchorMode::PointerAnchor => activity_target(time, targets)
            .or_else(|| targets.first().map(|t| t.point))
            .unwrap_or_else(|| cue.pinned_point.normalized()),
        ZoomAnchorMode::SmartAnchor => activity_target(time, targets).unwrap_or_else(|| cue.pinned_point.normalized()),
    }
}

/// Groups a cue's surviving input into viewport-sized activity regions, each
/// with one immutable center, so jitter cannot revise the destination.
fn activity_targets(cue: &ZoomCue, samples: &[PointerSample]) -> Vec<ActivityTarget> {
    let mut cue_samples: Vec<PointerSample> =
        samples.iter().copied().filter(|s| s.time >= cue.start && s.time <= cue.end).collect();
    if cue_samples.is_empty() {
        if let Some(preceding) = samples.iter().rev().find(|s| s.time < cue.start) {
            cue_samples = vec![*preceding];
        } else if let Some(following) = samples.iter().find(|s| s.time > cue.start) {
            cue_samples = vec![*following];
        }
    }
    let Some(&first) = cue_samples.first() else { return Vec::new() };
    let magnification = cue.zoom.max(1.0);
    let horizontal = 0.5 / magnification;
    let vertical = 0.7 / magnification;
    let mut group = ActivityGroup::new(first);
    let mut groups = Vec::new();
    for sample in cue_samples.iter().skip(1).copied() {
        if group.can_include(sample, horizontal, vertical) {
            group.include(sample);
        } else {
            groups.push(group);
            group = ActivityGroup::new(sample);
        }
    }
    groups.push(group);
    if cue.anchor_mode != ZoomAnchorMode::SmartAnchor {
        return groups
            .into_iter()
            .map(|g| ActivityTarget { activation_time: g.first_time, point: g.center() })
            .collect();
    }
    // With clicks, movement-only groups are transit between targets.
    let has_clicks = groups.iter().any(|g| g.first_press_time.is_some());
    groups
        .into_iter()
        .filter(|g| !has_clicks || g.first_press_time.is_some())
        .map(|g| ActivityTarget {
            activation_time: g.first_press_time.map(|t| (t - 0.3).max(cue.start)).unwrap_or(g.first_time),
            point: g.center(),
        })
        .collect()
}

fn activity_target(time: f64, targets: &[ActivityTarget]) -> Option<UnitPoint> {
    let first = targets.first()?;
    if time < first.activation_time {
        return None;
    }
    let index = targets.partition_point(|t| t.activation_time <= time);
    Some(targets[index.saturating_sub(1)].point)
}

/// Half-open boundaries, so an outgoing clip-end click never becomes a
/// target in the next clip.
fn is_retained(time: f64, clips: &ClipTimeline) -> bool {
    clips.segments.iter().any(|s| time >= s.source_start && time < s.source_end)
}

fn merged_pointer_samples(capture: &PointerCaptureFile) -> Vec<PointerSample> {
    let valid = |t: f64, x: f64, y: f64| t.is_finite() && (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y);
    let mut merged: Vec<(PointerSample, u8, usize)> = Vec::with_capacity(capture.travel.len() + capture.presses.len());
    for (index, travel) in capture.travel.iter().enumerate().filter(|(_, t)| valid(t.time, t.x, t.y)) {
        merged.push((
            PointerSample { time: travel.time, point: UnitPoint::new(travel.x, travel.y).normalized(), press: false },
            0,
            index,
        ));
    }
    for (index, press) in capture.presses.iter().enumerate().filter(|(_, p)| valid(p.time, p.x, p.y)) {
        merged.push((
            PointerSample {
                time: press.time,
                point: UnitPoint::new(press.x, press.y).normalized(),
                press: press.phase == PressPhase::Down,
            },
            1,
            index,
        ));
    }
    merged.sort_by(|a, b| {
        a.0.time.partial_cmp(&b.0.time).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2))
    });
    merged.into_iter().map(|(sample, _, _)| sample).collect()
}

fn bounded_anchor(point: UnitPoint, magnification: f64, mode: ZoomAnchorMode, bounds_bias: f64) -> UnitPoint {
    let raw = point.normalized();
    if mode == ZoomAnchorMode::PinnedAnchor {
        return clamp_to_frame(raw, magnification);
    }
    let half = 1.0 / (2.0 * magnification.max(1.0));
    // Keeps the subject at its original unzoomed screen position.
    let preserving = UnitPoint::new(half + raw.x * (1.0 - 2.0 * half), half + raw.y * (1.0 - 2.0 * half));
    let bias = unit(bounds_bias);
    let blended = UnitPoint::new(raw.x + (preserving.x - raw.x) * bias, raw.y + (preserving.y - raw.y) * bias);
    clamp_to_frame(blended, magnification)
}

/// The viewport never exposes space beyond the source.
fn clamp_to_frame(point: UnitPoint, magnification: f64) -> UnitPoint {
    let half = 1.0 / (2.0 * magnification.max(1.0));
    UnitPoint::new(point.x.clamp(half, 1.0 - half), point.y.clamp(half, 1.0 - half))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::manifest::{PointerPressEvent, PointerTravelSample};

    fn press(time: f64, x: f64, y: f64) -> PointerPressEvent {
        PointerPressEvent { time, x, y, button: 0, phase: PressPhase::Down, artwork_id: None }
    }

    fn travel(time: f64, x: f64, y: f64) -> PointerTravelSample {
        PointerTravelSample { time, x, y, kind: Default::default(), artwork_id: None }
    }

    fn samples(timeline: &ViewportTimeline) -> Vec<ViewportFrame> {
        (0..=(8.0 * 120.0) as usize).map(|i| timeline.frame(i as f64 / 120.0)).collect()
    }

    fn travel_center_of(frame: &ViewportFrame) -> (f64, f64) {
        let half = 1.0 / (2.0 * frame.magnification);
        ((frame.anchor.x - half) / (1.0 - 2.0 * half), (frame.anchor.y - half) / (1.0 - 2.0 * half))
    }

    /// Port of `checkZoomCamera` in Tests/ExportIntegration.swift.
    #[test]
    fn cap_style_zoom_rules() {
        let clips = ClipTimeline::full(8.0);
        let empty = ViewportTimeline::build(&[], &PointerCaptureFile::default(), &clips);
        assert!(samples(&empty).iter().all(|f| *f == ViewportFrame::IDENTITY), "no cues must never zoom");

        let pinned = ViewportTimeline::build(
            &[ZoomCue::new(1.0, 3.0).with_zoom(2.0).pinned(UnitPoint::new(0.7, 0.35))],
            &PointerCaptureFile::default(),
            &clips,
        );
        let aimed: Vec<ViewportFrame> = samples(&pinned).into_iter().filter(|f| f.magnification > 1.05).collect();
        assert!(!aimed.is_empty());
        assert!(
            aimed.iter().all(|f| {
                let (x, y) = travel_center_of(f);
                (x - 0.9).abs() < 0.0001 && (y - 0.2).abs() < 0.0001
            }),
            "zoom-in launches pre-aimed and zoom-out holds the last framing"
        );
        let settled = pinned.frame(2.9);
        assert!((settled.magnification - 2.0).abs() < 0.02);
        assert!((settled.anchor.x - 0.7).abs() < 0.01 && (settled.anchor.y - 0.35).abs() < 0.01);
        assert!((pinned.frame(6.0).magnification - 1.0).abs() < 0.01, "zoom-out returns to identity");

        let jitter = (0..100).map(|i| travel(0.5 + i as f64 * 0.02, 0.3 + if i % 2 == 0 { 0.04 } else { -0.04 }, 0.6));
        let corner = (0..100).map(|i| travel(2.6 + i as f64 * 0.02, 0.85 + if i % 2 == 0 { 0.02 } else { -0.02 }, 0.2));
        let capture = PointerCaptureFile {
            travel: jitter.chain(corner).collect(),
            presses: vec![press(1.0, 0.3, 0.6)],
            ..Default::default()
        };
        let follow_cues = vec![ZoomCue::new(0.5, 5.0).with_zoom(2.0)];
        let follow = ViewportTimeline::build(&follow_cues, &capture, &clips);
        let steady: Vec<UnitPoint> = (0..=60).map(|i| follow.frame(2.0 + i as f64 / 120.0).anchor).collect();
        assert!(
            steady.windows(2).all(|w| (w[0].x - w[1].x).abs() < 0.0001 && (w[0].y - w[1].y).abs() < 0.0001),
            "pointer jitter inside the dead zone must not move the Auto camera"
        );
        assert!(follow.frame(4.8).anchor.x > 0.65, "hovering into a corner re-aims without a click");

        let instant = ViewportTimeline::build(
            &[ZoomCue::new(1.0, 2.0).with_zoom(2.0).pinned(UnitPoint::CENTER).instant()],
            &PointerCaptureFile::default(),
            &clips,
        );
        assert!(
            instant.frame(1.0).magnification == 2.0 && instant.frame(2.05).magnification == 1.0,
            "instant zooms snap"
        );

        let mut mixed = follow_cues.clone();
        mixed.push(ZoomCue::new(5.2, 6.0).with_zoom(4.0).pinned(UnitPoint::new(0.0, 1.0)));
        mixed.push(ZoomCue::new(6.1, 7.5).with_zoom(3.0).with_anchor_mode(ZoomAnchorMode::SmartAnchor));
        let frames = samples(&ViewportTimeline::build(&mixed, &capture, &clips));
        assert!(
            frames.iter().all(|f| {
                let half = 1.0 / (2.0 * f.magnification);
                f.magnification >= 1.0
                    && f.anchor.x - half >= -1e-9
                    && f.anchor.x + half <= 1.0 + 1e-9
                    && f.anchor.y - half >= -1e-9
                    && f.anchor.y + half <= 1.0 + 1e-9
            }),
            "every sprung frame stays inside the source"
        );
        assert_eq!(frames, samples(&ViewportTimeline::build(&mixed, &capture, &clips)), "deterministic");

        let generated = synthesize_cues(
            &PointerCaptureFile { presses: vec![press(1.0, 0.4, 0.4), press(4.0, 0.6, 0.6)], ..Default::default() },
            10.0,
        );
        assert_eq!(generated.len(), 1);
        assert_eq!(generated[0].zoom, 2.0);
        assert!((generated[0].start - 0.7).abs() < 1e-9 && (generated[0].end - 6.5).abs() < 1e-9);
    }

    #[test]
    fn cues_decode_like_macos() {
        let cue: ZoomCue = serde_json::from_str(r#"{"start":1,"end":2}"#).unwrap();
        assert_eq!((cue.zoom, cue.anchor_mode, cue.is_enabled), (2.0, ZoomAnchorMode::PointerAnchor, true));
        let full: ZoomCue = serde_json::from_str(
            r#"{"id":"6BA7B810-9DAD-11D1-80B4-00C04FD430C8","start":1,"end":2,"zoom":3,"anchorMode":"pinnedAnchor","pinnedPoint":[0.25,0.75],"boundsBias":2,"isEnabled":false,"isImplicit":true,"skipsEasing":true}"#,
        )
        .unwrap();
        assert_eq!(full.pinned_point, UnitPoint::new(0.25, 0.75));
        assert_eq!(full.bounds_bias, 1.0, "bias is clamped to the unit range");
        let legacy: ZoomCue = serde_json::from_str(r#"{"start":1,"end":2,"anchorMode":"experimentalMode"}"#).unwrap();
        assert_eq!(legacy.anchor_mode, ZoomAnchorMode::PointerAnchor);
        let encoded = serde_json::to_value(&full).unwrap();
        assert_eq!(encoded["pinnedPoint"], serde_json::json!([0.25, 0.75]));
        assert_eq!(encoded["anchorMode"], "pinnedAnchor");
        assert_eq!(encoded["id"], "6BA7B810-9DAD-11D1-80B4-00C04FD430C8");
        let object_point: UnitPoint = serde_json::from_str(r#"{"x":0.1,"y":0.2}"#).unwrap();
        assert_eq!(object_point, UnitPoint::new(0.1, 0.2));
    }

    #[test]
    fn cuts_and_speed_do_not_change_the_camera_pace() {
        let cue = [ZoomCue::new(1.0, 3.0).with_zoom(2.0).pinned(UnitPoint::new(0.7, 0.35))];
        let slow = ClipTimeline::new(vec![crate::recording::clips::ClipSegment::with_speed(0.0, 8.0, 0.5)]);
        let timeline = ViewportTimeline::build(&cue, &PointerCaptureFile::default(), &slow);
        // Editor time 2 s is source time 1 s: the zoom starts there and eases at its own pace.
        assert!((timeline.frame(2.0).magnification - 1.0).abs() < 0.01);
        assert!(timeline.frame(2.5).magnification > 1.3 && timeline.frame(2.5).magnification < 2.0);
        let crop = UnitRect::new(0.5, 0.5, 0.5, 0.5);
        let cropped = timeline.cropped(&crop).frame(5.0);
        assert!(cropped.anchor.x >= 0.25 && cropped.anchor.x <= 0.75);
        assert_eq!(ViewportTimeline::identity().frame(3.0), ViewportFrame::IDENTITY);
    }
}
