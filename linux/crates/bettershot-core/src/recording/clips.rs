//! Port of `RecordingClipTimeline.swift`: the non-destructive edit map. The
//! source movie is never modified; each clip keeps a range on the original
//! recording, and the edited timeline is the concatenation of the surviving
//! ranges, each at its own speed.

use crate::capture_record::uppercase_uuid;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const EPSILON: f64 = 0.000_001;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipSegment {
    #[serde(with = "uppercase_uuid")]
    pub id: Uuid,
    pub source_start: f64,
    pub source_end: f64,
    /// Playback speed multiplier for this clip only (0.25...8).
    pub speed: f64,
}

impl ClipSegment {
    pub const MINIMUM_DURATION: f64 = 0.12;
    pub const MINIMUM_SPEED: f64 = 0.25;
    pub const MAXIMUM_SPEED: f64 = 8.0;

    pub fn new(source_start: f64, source_end: f64) -> Self {
        Self { id: Uuid::new_v4(), source_start, source_end, speed: 1.0 }
    }

    pub fn with_speed(source_start: f64, source_end: f64, speed: f64) -> Self {
        Self { id: Uuid::new_v4(), source_start, source_end, speed }
    }

    /// Raw duration of the source range this clip covers.
    pub fn duration(&self) -> f64 {
        (self.source_end - self.source_start).max(0.0)
    }

    /// How long the clip occupies on the edited timeline once its speed applies.
    pub fn editor_duration(&self) -> f64 {
        self.duration() / self.speed.max(Self::MINIMUM_SPEED)
    }
}

impl<'de> Deserialize<'de> for ClipSegment {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            #[serde(default)]
            id: Option<String>,
            source_start: Option<f64>,
            source_end: Option<f64>,
            // Accepted for migration from the first experimental split schema.
            start: Option<f64>,
            end: Option<f64>,
            speed: Option<f64>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let source_start =
            raw.source_start.or(raw.start).ok_or_else(|| serde::de::Error::missing_field("sourceStart"))?;
        let source_end = raw.source_end.or(raw.end).ok_or_else(|| serde::de::Error::missing_field("sourceEnd"))?;
        Ok(Self {
            id: raw.id.and_then(|id| Uuid::parse_str(&id).ok()).unwrap_or_else(Uuid::new_v4),
            source_start,
            source_end,
            speed: raw.speed.unwrap_or(1.0),
        })
    }
}

/// Where an editor time lands on the source.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub segment_index: usize,
    pub segment_id: Uuid,
    pub editor_start: f64,
    pub offset: f64,
    pub source_time: f64,
}

/// A removed source range and the edited boundary it collapsed to.
#[derive(Debug, Clone, PartialEq)]
pub struct CutMarker {
    pub source_start: f64,
    pub source_end: f64,
    pub editor_time: f64,
}

impl CutMarker {
    pub fn removed_duration(&self) -> f64 {
        (self.source_end - self.source_start).max(0.0)
    }
}

/// The part of a source range that survives, with its editor placement.
#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    pub segment_id: Uuid,
    pub source_start: f64,
    pub source_end: f64,
    pub editor_start: f64,
    pub editor_end: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ClipTimeline {
    pub segments: Vec<ClipSegment>,
}

impl ClipTimeline {
    pub fn new(segments: Vec<ClipSegment>) -> Self {
        Self { segments }
    }

    pub fn full(source_duration: f64) -> Self {
        let safe = if source_duration.is_finite() { source_duration.max(0.0) } else { 0.0 };
        if safe <= 0.0 {
            return Self::default();
        }
        Self::new(vec![ClipSegment::new(0.0, safe)])
    }

    pub fn legacy_trim(start: Option<f64>, end: Option<f64>, source_duration: f64) -> Self {
        let safe = if source_duration.is_finite() { source_duration.max(0.0) } else { 0.0 };
        let safe_start = start.unwrap_or(0.0).max(0.0).min(safe);
        let safe_end = end.unwrap_or(safe).max(safe_start).min(safe);
        if safe_end - safe_start < ClipSegment::MINIMUM_DURATION {
            return Self::full(safe);
        }
        Self::new(vec![ClipSegment::new(safe_start, safe_end)])
    }

    /// Total length of the edited timeline, honoring per-clip speed.
    pub fn duration(&self) -> f64 {
        self.segments.iter().map(ClipSegment::editor_duration).sum()
    }

    /// Every split has a marker; removed source gaps collapse to that boundary.
    pub fn cut_markers(&self, source_duration: f64) -> Vec<CutMarker> {
        if !source_duration.is_finite() || source_duration <= 0.0 {
            return Vec::new();
        }
        let mut markers = Vec::new();
        let mut source_end = 0.0;
        let mut editor_time = 0.0;
        for (index, segment) in self.segments.iter().enumerate() {
            if index > 0 || segment.source_start - source_end > EPSILON {
                markers.push(CutMarker { source_start: source_end, source_end: segment.source_start, editor_time });
            }
            source_end = segment.source_end;
            editor_time += segment.editor_duration();
        }
        if source_duration - source_end > EPSILON {
            markers.push(CutMarker { source_start: source_end, source_end: source_duration, editor_time });
        }
        markers
    }

    pub fn normalized(&self, source_duration: f64) -> Self {
        let safe = if source_duration.is_finite() { source_duration.max(0.0) } else { 0.0 };
        let mut seen = std::collections::HashSet::new();
        let mut normalized: Vec<ClipSegment> = self
            .segments
            .iter()
            .filter_map(|segment| {
                let start = segment.source_start.max(0.0).min(safe);
                let end = segment.source_end.max(start).min(safe);
                if end - start < ClipSegment::MINIMUM_DURATION {
                    return None;
                }
                let id = if seen.insert(segment.id) { segment.id } else { Uuid::new_v4() };
                let speed = if segment.speed.is_finite() { segment.speed } else { 1.0 };
                let speed = speed.clamp(ClipSegment::MINIMUM_SPEED, ClipSegment::MAXIMUM_SPEED);
                Some(ClipSegment { id, source_start: start, source_end: end, speed })
            })
            .collect();
        normalized.sort_by(|a, b| {
            if (a.source_start - b.source_start).abs() > EPSILON {
                a.source_start.partial_cmp(&b.source_start).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                a.source_end.partial_cmp(&b.source_end).unwrap_or(std::cmp::Ordering::Equal)
            }
        });
        // A corrupt or old project must never overlap: keep the first range
        // and trim later ranges forward.
        let mut previous_end = 0.0;
        let normalized: Vec<ClipSegment> = normalized
            .into_iter()
            .filter_map(|mut segment| {
                segment.source_start = segment.source_start.max(previous_end);
                if segment.duration() < ClipSegment::MINIMUM_DURATION {
                    return None;
                }
                previous_end = segment.source_end;
                Some(segment)
            })
            .collect();
        if normalized.is_empty() && safe > 0.0 {
            return Self::full(safe);
        }
        Self::new(normalized)
    }

    pub fn location(&self, editor_time: f64) -> Option<Location> {
        let duration = self.duration();
        if self.segments.is_empty() || duration <= 0.0 {
            return None;
        }
        let clamped = editor_time.max(0.0).min(duration);
        let mut editor_start = 0.0;
        let last = self.segments.len() - 1;
        for (index, segment) in self.segments.iter().enumerate() {
            let editor_end = editor_start + segment.editor_duration();
            if clamped < editor_end || index == last {
                // A sped-up segment covers more source seconds per editor second.
                let offset = (clamped - editor_start).max(0.0).min(segment.editor_duration());
                let source_offset = (offset * segment.speed).min(segment.duration());
                return Some(Location {
                    segment_index: index,
                    segment_id: segment.id,
                    editor_start,
                    offset,
                    source_time: segment.source_start + source_offset,
                });
            }
            editor_start = editor_end;
        }
        None
    }

    pub fn source_time(&self, editor_time: f64) -> f64 {
        self.location(editor_time).map(|l| l.source_time).unwrap_or(0.0)
    }

    pub fn editor_time(&self, source_time: f64) -> Option<f64> {
        let mut editor_start = 0.0;
        for segment in &self.segments {
            if source_time >= segment.source_start - EPSILON && source_time <= segment.source_end + EPSILON {
                let source_offset = (source_time - segment.source_start).max(0.0).min(segment.duration());
                return Some(editor_start + source_offset / segment.speed);
            }
            editor_start += segment.editor_duration();
        }
        None
    }

    pub fn editor_range(&self, segment_id: Uuid) -> Option<(f64, f64)> {
        let mut editor_start = 0.0;
        for segment in &self.segments {
            let editor_end = editor_start + segment.editor_duration();
            if segment.id == segment_id {
                return Some((editor_start, editor_end));
            }
            editor_start = editor_end;
        }
        None
    }

    /// Splits the clip under `editor_time`; the trailing half gets a new id,
    /// which is returned for selection.
    pub fn split(&self, editor_time: f64) -> Option<(ClipTimeline, Uuid)> {
        let location = self.location(editor_time)?;
        let segment = &self.segments[location.segment_index];
        let source_time = location.source_time;
        if source_time - segment.source_start < ClipSegment::MINIMUM_DURATION
            || segment.source_end - source_time < ClipSegment::MINIMUM_DURATION
        {
            return None;
        }
        let trailing_id = Uuid::new_v4();
        let leading = ClipSegment {
            id: segment.id,
            source_start: segment.source_start,
            source_end: source_time,
            speed: segment.speed,
        };
        let trailing = ClipSegment {
            id: trailing_id,
            source_start: source_time,
            source_end: segment.source_end,
            speed: segment.speed,
        };
        let mut next = self.segments.clone();
        next.splice(location.segment_index..=location.segment_index, [leading, trailing]);
        Some((Self::new(next), trailing_id))
    }

    pub fn deleting(&self, segment_id: Uuid) -> Option<ClipTimeline> {
        if self.segments.len() <= 1 || !self.segments.iter().any(|s| s.id == segment_id) {
            return None;
        }
        let next: Vec<ClipSegment> = self.segments.iter().filter(|s| s.id != segment_id).cloned().collect();
        if next.iter().map(ClipSegment::duration).sum::<f64>() < ClipSegment::MINIMUM_DURATION {
            return None;
        }
        Some(Self::new(next))
    }

    pub fn replacing(&self, replacement: ClipSegment) -> ClipTimeline {
        let Some(index) = self.segments.iter().position(|s| s.id == replacement.id) else { return self.clone() };
        let mut next = self.segments.clone();
        next[index] = replacement;
        Self::new(next)
    }

    /// Cuts source ranges out, splitting segments where a range lands inside
    /// one. Returns None when nothing playable would survive.
    pub fn removing_source_ranges(&self, ranges: &[(f64, f64)]) -> Option<ClipTimeline> {
        let cuts = Self::merged_ranges(ranges);
        if cuts.is_empty() {
            return Some(self.clone());
        }
        let mut next = Vec::new();
        for segment in &self.segments {
            let mut pieces: Vec<(f64, f64)> = Vec::new();
            let mut cursor = segment.source_start;
            for cut in cuts.iter().filter(|c| c.1 > segment.source_start && c.0 < segment.source_end) {
                if cut.0 > cursor {
                    pieces.push((cursor, cut.0.min(segment.source_end)));
                }
                cursor = cursor.max(cut.1);
            }
            if cursor < segment.source_end {
                pieces.push((cursor, segment.source_end));
            }
            let mut kept_id = false;
            for piece in pieces.into_iter().filter(|p| p.1 - p.0 >= ClipSegment::MINIMUM_DURATION) {
                next.push(ClipSegment {
                    id: if kept_id { Uuid::new_v4() } else { segment.id },
                    source_start: piece.0,
                    source_end: piece.1,
                    speed: segment.speed,
                });
                kept_id = true;
            }
        }
        (!next.is_empty()).then(|| Self::new(next))
    }

    /// Overlapping or touching ranges collapsed into disjoint sorted ranges.
    pub fn merged_ranges(ranges: &[(f64, f64)]) -> Vec<(f64, f64)> {
        let mut sorted: Vec<(f64, f64)> =
            ranges.iter().copied().filter(|r| r.0.is_finite() && r.1.is_finite() && r.1 > r.0).collect();
        sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for range in sorted {
            match merged.last_mut() {
                Some(last) if range.0 <= last.1 => last.1 = last.1.max(range.1),
                _ => merged.push(range),
            }
        }
        merged
    }

    pub fn slices(&self, source_start: f64, source_end: f64) -> Vec<Slice> {
        if source_end <= source_start {
            return Vec::new();
        }
        let mut result = Vec::new();
        let mut editor_offset = 0.0;
        for segment in &self.segments {
            let overlap_start = source_start.max(segment.source_start);
            let overlap_end = source_end.min(segment.source_end);
            if overlap_end > overlap_start {
                result.push(Slice {
                    segment_id: segment.id,
                    source_start: overlap_start,
                    source_end: overlap_end,
                    editor_start: editor_offset + (overlap_start - segment.source_start) / segment.speed,
                    editor_end: editor_offset + (overlap_end - segment.source_start) / segment.speed,
                });
            }
            editor_offset += segment.editor_duration();
        }
        result
    }

    pub fn is_unedited(&self, source_duration: f64) -> bool {
        match self.segments.as_slice() {
            [only] => {
                only.source_start.abs() < EPSILON
                    && (only.source_end - source_duration).abs() < EPSILON
                    && (only.speed - 1.0).abs() < EPSILON
            }
            _ => false,
        }
    }
}

/// Port of `RecordingClipSpeedDraft`: holds a Speed slider drag until release,
/// so one undo step is registered per drag instead of per tick.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClipSpeedDraft {
    clip_id: Option<Uuid>,
    speed: Option<f64>,
    editing: bool,
}

impl ClipSpeedDraft {
    pub fn begin(&mut self) {
        self.editing = true;
    }

    /// True when the change should apply now rather than on `end`.
    pub fn propose(&mut self, speed: f64, clip_id: Uuid) -> bool {
        if !self.editing {
            return true;
        }
        if self.clip_id.is_some_and(|id| id != clip_id) {
            return false;
        }
        self.clip_id = Some(clip_id);
        self.speed = Some(speed);
        false
    }

    /// Ends the edit, returning the held change to apply once.
    pub fn end(&mut self) -> Option<(Uuid, f64)> {
        let result = self.clip_id.zip(self.speed);
        *self = Self::default();
        result
    }

    pub fn speed_for(&self, clip_id: Uuid) -> Option<f64> {
        (self.clip_id == Some(clip_id)).then_some(self.speed).flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < EPSILON
    }

    #[test]
    fn fractional_speeds_time_split_and_persist() {
        // Port of RecordingClipSpeedCheck: a 10-second source keeps fractional rates.
        for (speed, duration) in [(0.25, 40.0), (0.5, 20.0), (1.25, 8.0), (1.5, 20.0 / 3.0), (2.5, 4.0)] {
            let timeline = ClipTimeline::new(vec![ClipSegment::with_speed(0.0, 10.0, speed)]).normalized(10.0);
            assert!(close(timeline.duration(), duration), "{speed}× must produce {duration} s");
            let json = serde_json::to_string(&timeline).unwrap();
            let restored: ClipTimeline = serde_json::from_str(&json).unwrap();
            assert_eq!(restored.normalized(10.0), timeline);
            assert!(close(timeline.source_time(duration / 2.0), 5.0));
            assert!(close(timeline.editor_time(5.0).unwrap(), duration / 2.0));
            let (split, trailing) = timeline.split(duration / 2.0).unwrap();
            assert!(split.segments.iter().all(|s| s.speed == speed));
            assert!(close(split.duration(), duration));
            assert_eq!(split.segments[1].id, trailing);
            assert_eq!(split.segments[0].id, timeline.segments[0].id, "the leading half keeps its identity");
        }
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let timeline = ClipTimeline::new(vec![ClipSegment::with_speed(0.0, 10.0, invalid)]).normalized(10.0);
            assert!(timeline.duration().is_finite() && timeline.duration() > 0.0);
        }
        let legacy: ClipSegment = serde_json::from_str(r#"{"sourceStart":0,"sourceEnd":10}"#).unwrap();
        assert!(legacy.speed == 1.0 && legacy.editor_duration() == 10.0);
        let experimental: ClipSegment = serde_json::from_str(r#"{"start":1,"end":4,"speed":2}"#).unwrap();
        assert_eq!((experimental.source_start, experimental.source_end, experimental.speed), (1.0, 4.0, 2.0));
        let encoded = serde_json::to_value(&legacy).unwrap();
        assert_eq!(encoded["id"].as_str().unwrap(), encoded["id"].as_str().unwrap().to_uppercase());
        assert_eq!(encoded["speed"], 1.0);
    }

    #[test]
    fn speed_drafts_apply_once_on_release() {
        let clip = Uuid::new_v4();
        let mut draft = ClipSpeedDraft::default();
        draft.begin();
        for speed in [1.25, 1.5, 1.75, 2.0] {
            assert!(!draft.propose(speed, clip), "a drag tick must not apply");
            assert_eq!(draft.speed_for(clip), Some(speed));
        }
        assert_eq!(draft.end(), Some((clip, 2.0)));
        assert_eq!(draft.speed_for(clip), None);
        assert_eq!(draft.end(), None);
        assert!(draft.propose(3.0, clip), "outside a drag a change applies immediately");
        let other = Uuid::new_v4();
        let mut retarget = ClipSpeedDraft::default();
        retarget.begin();
        assert!(!retarget.propose(1.5, clip));
        assert!(!retarget.propose(1.75, other));
        assert_eq!(retarget.speed_for(other), None);
        assert_eq!(retarget.speed_for(clip), Some(1.5));
        assert_eq!(retarget.end(), Some((clip, 1.5)));
    }

    #[test]
    fn cuts_markers_and_source_ranges() {
        let full = ClipTimeline::full(10.0);
        assert!(full.is_unedited(10.0));
        assert!(full.cut_markers(10.0).is_empty());
        let (split, _) = full.split(4.0).unwrap();
        let deleted = split.deleting(split.segments[0].id).unwrap();
        assert_eq!(deleted.segments.len(), 1);
        assert!(close(deleted.segments[0].source_start, 4.0));
        let markers = deleted.cut_markers(10.0);
        assert_eq!(markers.len(), 1);
        assert!(close(markers[0].removed_duration(), 4.0) && close(markers[0].editor_time, 0.0));
        assert!(!deleted.is_unedited(10.0));
        assert_eq!(full.deleting(full.segments[0].id), None, "the last clip cannot be deleted");

        let cut = full.removing_source_ranges(&[(2.0, 3.0), (2.5, 4.0), (9.95, 20.0)]).unwrap();
        let ranges: Vec<(f64, f64)> = cut.segments.iter().map(|s| (s.source_start, s.source_end)).collect();
        assert!(close(ranges[0].0, 0.0) && close(ranges[0].1, 2.0));
        assert!(close(ranges[1].0, 4.0) && close(ranges[1].1, 9.95));
        assert_eq!(cut.segments.len(), 2, "a leftover shorter than the minimum is dropped");
        assert_eq!(cut.segments[0].id, full.segments[0].id);
        assert_eq!(full.removing_source_ranges(&[(0.0, 10.0)]), None);
        assert_eq!(
            ClipTimeline::merged_ranges(&[(5.0, 6.0), (1.0, 2.0), (1.5, 3.0), (f64::NAN, 1.0), (4.0, 4.0)]),
            vec![(1.0, 3.0), (5.0, 6.0)]
        );
        let slices = cut.slices(1.0, 5.0);
        assert_eq!(slices.len(), 2);
        assert!(close(slices[1].editor_start, 2.0) && close(slices[1].editor_end, 3.0));
        assert_eq!(cut.editor_time(3.0), None, "a cut source time has no editor time");
        assert!(close(cut.editor_time(4.0).unwrap(), 2.0));
        assert!(close(cut.source_time(2.5), 4.5));
        assert_eq!(cut.editor_range(cut.segments[1].id).map(|(s, _)| s), Some(2.0));
    }

    #[test]
    fn normalization_repairs_old_projects() {
        let messy = ClipTimeline::new(vec![
            ClipSegment::with_speed(5.0, 20.0, 1.0),
            ClipSegment::with_speed(-1.0, 6.0, 0.1),
            ClipSegment::with_speed(3.0, 3.05, 1.0),
        ])
        .normalized(10.0);
        assert_eq!(messy.segments.len(), 2);
        assert!(close(messy.segments[0].source_start, 0.0) && close(messy.segments[0].source_end, 6.0));
        assert_eq!(messy.segments[0].speed, ClipSegment::MINIMUM_SPEED);
        assert!(close(messy.segments[1].source_start, 6.0) && close(messy.segments[1].source_end, 10.0));
        let refilled = ClipTimeline::new(vec![]).normalized(10.0);
        assert!(refilled.is_unedited(10.0), "an empty list becomes the full source");
        assert_eq!(ClipTimeline::legacy_trim(Some(2.0), Some(8.0), 10.0).segments[0].source_end, 8.0);
        assert!(ClipTimeline::legacy_trim(Some(2.0), Some(2.05), 10.0).is_unedited(10.0));
        assert!(ClipTimeline::full(f64::NAN).segments.is_empty());
        assert_eq!(ClipTimeline::full(0.0).location(1.0), None);
        let duplicate = ClipSegment::new(0.0, 2.0);
        let mut twin = duplicate.clone();
        twin.source_start = 3.0;
        twin.source_end = 5.0;
        let repaired = ClipTimeline::new(vec![duplicate.clone(), twin]).normalized(10.0);
        assert_ne!(repaired.segments[0].id, repaired.segments[1].id, "duplicate ids are reminted");
    }
}
