//! The studio's state, a port of the document, undo, timeline, and render
//! parts of `RecordingStudioModel.swift`: the project's style, clips, zoom
//! cues, crop, and masks with undo and redo; the playhead; the frame readers
//! and the shared compositor the preview draws through; and the save and
//! export requests. Everything the window shows comes from here.

use bettershot_core::recording::edit::{
    ExportAspectContentMode, ExportAspectPreset, KeystrokePlacement, MaskEffect, MaskSegment, RecordingEditDocument,
    StudioStyle, SubtitleBarStyle, SubtitleCue, TranscriptWord, UnitRect, VideoCompressionSettings,
};
use bettershot_core::recording::pointer::{self as pointer, PointerTimelineOptions};
use bettershot_core::recording::viewport::{UnitPoint, ZoomAnchorMode};
use bettershot_core::recording::{
    self, deliverable, ClipSegment, ClipTimeline, ContentMode, KaraokeTimeline, KeystrokeCaptionTimeline,
    PointerCaptureFile, PointerTimeline, Preset3D, RecordingSession, ReframeTrack, Shot3D, StudioLayout,
    SubtitleTimeline, Timeline3D, ViewportTimeline, ZoomCue,
};
use bettershot_platform::video::{self, ExportRequest, FrameReader};
use bettershot_render::studio::StudioCompositor;
use std::path::PathBuf;
use uuid::Uuid;

/// Everything undo restores.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub style: StudioStyle,
    pub clips: ClipTimeline,
    pub zoom_cues: Vec<ZoomCue>,
    pub zoom_enabled: bool,
    pub crop: UnitRect,
    pub masks: Vec<MaskSegment>,
    pub export_settings: VideoCompressionSettings,
    /// Click pulses over the synthetic pointer (`showsClickEffects`).
    pub shows_click_effects: bool,
    /// The output aspect and how the recording fills it.
    pub export_aspect: ExportAspectPreset,
    pub export_aspect_mode: ExportAspectContentMode,
    pub shows_keystrokes: bool,
    pub keystroke_placement: KeystrokePlacement,
    pub shows_subtitles: bool,
    pub subtitle_style: SubtitleBarStyle,
    /// The narration transcript (from macOS, or whisper.cpp here).
    pub subtitle_cues: Vec<SubtitleCue>,
    pub subtitle_words: Vec<TranscriptWord>,
    /// 3D shots on the edited timeline.
    pub shots_3d: Vec<Shot3D>,
}

pub struct StudioModel {
    pub session: RecordingSession,
    pub source_duration: f64,
    pub source_size: (u32, u32),
    pub has_camera: bool,
    /// Whether the recording has audio tracks to play and transcribe.
    pub has_audio: bool,
    pub camera_lead_in: f64,
    /// True when the capture hid the system cursor, so the studio draws it
    /// from `input.json` and can restyle it.
    pub pointer_synthesized: bool,
    /// The recording's size in points, for the pointer springs' thresholds.
    recording_point_size: (f64, f64),
    /// The loaded document, which keeps fields this build does not edit
    /// (subtitles, 3D shots) across saves.
    base_document: RecordingEditDocument,
    pub state: Snapshot,
    saved: Snapshot,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    pub playhead: f64,
    pub selected_clip: Option<Uuid>,
    pub selected_cue: Option<Uuid>,
    pub selected_mask: Option<Uuid>,
    pub selected_shot: Option<String>,
    pub scissors: bool,
    /// While the crop is adjusted on the preview: the crop before editing
    /// began and the undo depth then, so cancelling restores both.
    crop_editing: Option<(UnitRect, usize)>,
    pub viewport: ViewportTimeline,
    pub pointer: PointerTimeline,
    /// Captions from the recorded chords and the project's subtitles.
    keystrokes: KeystrokeCaptionTimeline,
    subtitles: SubtitleTimeline,
    karaoke: KaraokeTimeline,
    pub screen: FrameReader,
    pub camera: Option<FrameReader>,
    pub compositor: StudioCompositor,
    capture: PointerCaptureFile,
}

impl StudioModel {
    pub const MAX_UNDO: usize = 100;

    pub fn load(session: RecordingSession, default_style: StudioStyle) -> Result<Self, String> {
        let manifest = session.load_capture_manifest();
        let screen = FrameReader::open(&session.screen_path())?;
        let source_duration = manifest.as_ref().map(|m| m.duration).filter(|d| *d > 0.0).unwrap_or(screen.duration);
        let source_size = match manifest.as_ref() {
            Some(m) if m.pixel_width > 0 && m.pixel_height > 0 => (m.pixel_width as u32, m.pixel_height as u32),
            _ => (screen.width, screen.height),
        };
        let has_camera = session.has_camera();
        let has_audio = manifest.as_ref().is_some_and(|m| m.includes_system_audio || m.includes_microphone);
        let camera = if has_camera { FrameReader::open(&session.camera_path()).ok() } else { None };
        let camera_lead_in = manifest.as_ref().and_then(|m| m.camera_lead_in).unwrap_or(0.0);
        let pointer_synthesized = manifest.as_ref().is_some_and(|m| !m.pointer_in_pixels());
        let press_effects_default = manifest.as_ref().and_then(|m| m.press_effects_enabled).unwrap_or(true);
        let pixel_scale = manifest.as_ref().map(|m| m.pixel_scale).unwrap_or(1.0).max(1.0);
        let recording_point_size = (f64::from(source_size.0) / pixel_scale, f64::from(source_size.1) / pixel_scale);
        let capture = session.load_pointer_capture().unwrap_or_default();
        let loaded = session.effective_edit_document().and_then(|value| RecordingEditDocument::from_value(&value));
        let snapshot_of = |document: &RecordingEditDocument| Snapshot {
            style: document.style_value(),
            clips: document.clip_timeline(source_duration),
            zoom_cues: document.zoom_cues.clone(),
            zoom_enabled: document.zoom_enabled,
            crop: document.crop_value(),
            masks: document.masks_value(),
            export_settings: document.export_settings_value(),
            shows_click_effects: document.shows_click_effects.unwrap_or(press_effects_default),
            export_aspect: document.export_aspect_preset(),
            export_aspect_mode: document.export_aspect_content_mode(),
            shows_keystrokes: document.shows_keystrokes.unwrap_or(true),
            keystroke_placement: document.keystroke_placement.unwrap_or_default(),
            shows_subtitles: document.shows_subtitles.unwrap_or(true),
            subtitle_style: document.subtitle_style(),
            subtitle_cues: document.subtitle_cues.clone().unwrap_or_default(),
            subtitle_words: document.subtitle_words.clone().unwrap_or_default(),
            shots_3d: Timeline3D::parse(document.shots_3d.as_deref().unwrap_or_default()),
        };
        let state = match &loaded {
            Some(document) => snapshot_of(document),
            None => Snapshot {
                style: StudioStyle { camera: Default::default(), ..default_style },
                clips: ClipTimeline::full(source_duration),
                zoom_cues: recording::synthesize_cues(&capture, source_duration),
                zoom_enabled: true,
                crop: UnitRect::UNIT,
                masks: Vec::new(),
                export_settings: VideoCompressionSettings::default(),
                shows_click_effects: press_effects_default,
                export_aspect: ExportAspectPreset::Original,
                export_aspect_mode: ExportAspectContentMode::Fill,
                shows_keystrokes: true,
                keystroke_placement: KeystrokePlacement::default(),
                shows_subtitles: true,
                subtitle_style: SubtitleBarStyle::default(),
                subtitle_cues: Vec::new(),
                subtitle_words: Vec::new(),
                shots_3d: Vec::new(),
            },
        };
        let base_document = loaded.clone().unwrap_or_else(|| RecordingEditDocument::new(&state.style, &state.clips));
        let saved = match loaded.as_ref().and_then(|_| session.load_edit_document()) {
            // Only an explicit save counts as the saved state; a draft is dirty.
            Some(value) => {
                RecordingEditDocument::from_value(&value).map(|d| snapshot_of(&d)).unwrap_or_else(|| state.clone())
            }
            None => state.clone(),
        };
        let keystrokes = KeystrokeCaptionTimeline::new(&capture.keystrokes);
        let subtitles = SubtitleTimeline::new(&state.subtitle_cues);
        let karaoke = KaraokeTimeline::new(&state.subtitle_cues, &state.subtitle_words);
        let pointer = Self::build_pointer(&state, &capture, source_duration, recording_point_size, pointer_synthesized);
        let viewport = Self::build_viewport(
            &state,
            &capture,
            source_size,
            source_duration,
            recording_point_size,
            &pointer,
            pointer_synthesized,
        );
        let overlays = Overlays { keystrokes: &keystrokes, subtitles: &subtitles, karaoke: &karaoke };
        let compositor = Self::build_compositor(
            &state,
            source_size,
            has_camera,
            &viewport,
            &pointer,
            pointer_synthesized,
            false,
            overlays,
        );
        Ok(Self {
            session,
            source_duration,
            source_size,
            has_camera,
            has_audio,
            camera_lead_in,
            pointer_synthesized,
            recording_point_size,
            base_document,
            state,
            saved,
            undo: Vec::new(),
            redo: Vec::new(),
            playhead: 0.0,
            selected_clip: None,
            selected_cue: None,
            selected_mask: None,
            selected_shot: None,
            scissors: false,
            crop_editing: None,
            viewport,
            pointer,
            keystrokes,
            subtitles,
            karaoke,
            screen,
            camera,
            compositor,
            capture,
        })
    }

    // MARK: - Derived

    /// The cropped source in even pixels (`croppedVideoSize`), the basis of
    /// every output canvas.
    fn cropped_size(state: &Snapshot, source_size: (u32, u32)) -> (f64, f64) {
        state.crop.cropped_size((f64::from(source_size.0), f64::from(source_size.1)))
    }

    /// The output canvas: the cropped source size, or the reframe preset's.
    pub fn canvas_size(&self) -> (f64, f64) {
        Self::canvas_for(&self.state, self.source_size)
    }

    fn canvas_for(state: &Snapshot, source_size: (u32, u32)) -> (f64, f64) {
        state.export_aspect.canvas_size(Self::cropped_size(state, source_size))
    }

    /// The layout for `canvas`: a reframe aspect-fills or fits the cropped
    /// source into the card, as `makeExportConfiguration` arranges it.
    fn make_layout(state: &Snapshot, source_size: (u32, u32), has_camera: bool, canvas: (f64, f64)) -> StudioLayout {
        let cropped = Self::cropped_size(state, source_size);
        let reframed = state.export_aspect.ratio().is_some();
        let fit = reframed && state.export_aspect_mode == ExportAspectContentMode::Fit;
        let aspect = (reframed && cropped.1 > 0.0).then_some(cropped.0 / cropped.1);
        StudioLayout::make(
            canvas,
            &state.style,
            has_camera,
            aspect,
            if fit { ContentMode::Fit } else { ContentMode::Fill },
        )
    }

    /// The virtual camera: the zoom viewport through the crop, or, when
    /// exporting into another aspect with Fill, the crop-and-follow reframe
    /// camera built on top of it (`makeReframeTrack`).
    fn build_viewport(
        state: &Snapshot,
        capture: &PointerCaptureFile,
        source_size: (u32, u32),
        source_duration: f64,
        recording_point_size: (f64, f64),
        pointer: &PointerTimeline,
        synthesized: bool,
    ) -> ViewportTimeline {
        let zoomed = if state.zoom_enabled {
            ViewportTimeline::build(&state.zoom_cues, capture, &state.clips)
        } else {
            ViewportTimeline::identity()
        };
        let cropped = zoomed.cropped(&state.crop);
        if state.export_aspect.ratio().is_none() || state.export_aspect_mode != ExportAspectContentMode::Fill {
            return cropped;
        }
        // Focus comes from the recorded pointer whenever the session
        // captured one, whether or not the cursor is drawn synthetically.
        let focus = if synthesized {
            Some(pointer.clone())
        } else if capture.has_pointer_data() {
            Some(PointerTimeline::build(
                capture,
                source_duration,
                PointerTimelineOptions {
                    recording_size_in_points: recording_point_size,
                    clips: Some(&state.clips),
                    ..Default::default()
                },
            ))
        } else {
            None
        };
        let crop = state.crop;
        ReframeTrack::build(
            state.export_aspect,
            Self::cropped_size(state, source_size),
            &cropped,
            state.clips.duration(),
            |time| {
                let location = focus.as_ref()?.location(time)?;
                let remapped = crop.point(location);
                Some(UnitPoint::new(remapped.x.clamp(0.0, 1.0), remapped.y.clamp(0.0, 1.0)))
            },
        )
        .map(|track| track.timeline)
        .unwrap_or(cropped)
    }

    /// Port of `rebuildPointerTimeline`: the smoothed cursor along the
    /// edited timeline, with the chosen style drawn instead of the
    /// recorded artwork.
    fn build_pointer(
        state: &Snapshot,
        capture: &PointerCaptureFile,
        source_duration: f64,
        recording_point_size: (f64, f64),
        synthesized: bool,
    ) -> PointerTimeline {
        if !synthesized || source_duration <= 0.0 {
            return PointerTimeline::empty();
        }
        PointerTimeline::build(
            capture,
            source_duration,
            PointerTimelineOptions {
                recording_size_in_points: recording_point_size,
                fallback_artwork: Some(pointer::default_artwork()),
                hide_after_inactivity: None,
                clips: Some(&state.clips),
                cursor: state.style.cursor.clone(),
                override_artwork: pointer::styled_artwork(state.style.cursor.appearance),
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_compositor(
        state: &Snapshot,
        source_size: (u32, u32),
        has_camera: bool,
        viewport: &ViewportTimeline,
        pointer: &PointerTimeline,
        synthesized: bool,
        editing_crop: bool,
        overlays: Overlays<'_>,
    ) -> StudioCompositor {
        // Crop editing shows the whole, unzoomed source so its handles stay
        // in source coordinates, as the macOS flat player does.
        let (viewport, crop, layout) = if editing_crop {
            let uncropped =
                Snapshot { crop: UnitRect::UNIT, export_aspect: ExportAspectPreset::Original, ..state.clone() };
            let canvas = Self::canvas_for(&uncropped, source_size);
            (
                ViewportTimeline::identity(),
                UnitRect::UNIT,
                Self::make_layout(&uncropped, source_size, has_camera, canvas),
            )
        } else {
            let canvas = Self::canvas_for(state, source_size);
            (viewport.clone(), state.crop, Self::make_layout(state, source_size, has_camera, canvas))
        };
        let shots =
            if editing_crop { Timeline3D::empty() } else { Timeline3D::new(&state.shots_3d, state.clips.duration()) };
        StudioCompositor::new(layout.canvas_size, state.style.clone(), layout, viewport, crop, state.masks.clone())
            .with_pointer(synthesized.then(|| pointer.clone()), state.shows_click_effects)
            .with_overlays(Some(state.clips.clone()), overlays.keystrokes_for(state), overlays.subtitles_for(state))
            .with_shots_3d(shots, 0.5)
    }

    // MARK: - 3D shots

    /// The normalized shots, as the compositor and the lane use them.
    pub fn timeline_3d(&self) -> Timeline3D {
        Timeline3D::new(&self.state.shots_3d, self.duration())
    }

    /// Adds a shot in the free span at the playhead (`insertionRange`) with
    /// `preset`; false when the playhead sits on a shot or in a gap too short.
    pub fn add_shot_at_playhead(&mut self, preset: Preset3D) -> bool {
        let timeline = self.timeline_3d();
        let Some((start, end)) = timeline.insertion_range(self.playhead, self.duration()) else { return false };
        let mut shot = Shot3D::new(start, end);
        shot.apply(preset);
        let id = shot.id.clone();
        self.edit(|s| s.shots_3d.push(shot));
        self.selected_shot = Some(id);
        true
    }

    /// Replaces every shot with Cap's automatic scene of `count` shots.
    pub fn apply_auto_scene(&mut self, count: usize) {
        let cuts: Vec<f64> = self.state.clips.cut_markers(self.source_duration).iter().map(|m| m.editor_time).collect();
        let shots = Timeline3D::auto_scene(count, self.duration(), &cuts);
        self.edit(|s| s.shots_3d = shots);
        self.selected_shot = None;
    }

    pub fn remove_shot(&mut self, id: &str) {
        let id = id.to_owned();
        self.edit(|s| s.shots_3d.retain(|shot| shot.id != id));
    }

    pub fn update_shot(&mut self, shot: Shot3D, coalesced: bool) {
        let apply = |s: &mut Snapshot| {
            if let Some(slot) = s.shots_3d.iter_mut().find(|existing| existing.id == shot.id) {
                *slot = shot;
            }
        };
        if coalesced {
            self.edit_coalesced(apply);
        } else {
            self.edit(apply);
        }
    }

    pub fn shot_at_playhead(&self) -> Option<String> {
        self.timeline_3d().shot(self.playhead).map(|s| s.id.clone())
    }

    pub fn has_keystrokes(&self) -> bool {
        !self.keystrokes.is_empty()
    }

    pub fn has_subtitles(&self) -> bool {
        !self.subtitles.is_empty()
    }

    pub fn has_word_timings(&self) -> bool {
        !self.karaoke.is_empty()
    }

    // MARK: - Crop editing on the preview

    pub fn is_editing_crop(&self) -> bool {
        self.crop_editing.is_some()
    }

    /// Shows the full source with crop handles; edits until Apply are one
    /// undo step, and Cancel restores the crop that was applied before.
    pub fn begin_crop_edit(&mut self) {
        if self.crop_editing.is_none() {
            self.crop_editing = Some((self.state.crop, self.undo.len()));
            self.invalidate();
        }
    }

    pub fn apply_crop_edit(&mut self) {
        if self.crop_editing.take().is_some() {
            self.invalidate();
        }
    }

    /// Port of "clicking Crop again cancels the uncommitted crop": the
    /// previously applied crop survives.
    pub fn cancel_crop_edit(&mut self) {
        if let Some((before, undo_len)) = self.crop_editing.take() {
            self.state.crop = before;
            self.undo.truncate(undo_len);
            self.invalidate();
        }
    }

    /// Whether the pointer can be restyled: the capture hid the cursor.
    pub fn can_style_pointer(&self) -> bool {
        self.pointer_synthesized
    }

    pub fn duration(&self) -> f64 {
        self.state.clips.duration()
    }

    pub fn source_time(&self, editor_time: f64) -> f64 {
        self.state.clips.source_time(editor_time)
    }

    pub fn is_dirty(&self) -> bool {
        self.state != self.saved
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Replaces the transcript (None removes it), as one undo step.
    pub fn set_transcript(&mut self, cues: Vec<SubtitleCue>, words: Vec<TranscriptWord>) {
        self.edit(|s| {
            s.subtitle_cues = cues;
            s.subtitle_words = words;
            s.shows_subtitles = true;
        });
    }

    /// Rebuilds the camera path, pointer, and compositor after an edit.
    fn invalidate(&mut self) {
        self.subtitles = SubtitleTimeline::new(&self.state.subtitle_cues);
        self.karaoke = KaraokeTimeline::new(&self.state.subtitle_cues, &self.state.subtitle_words);
        self.pointer = Self::build_pointer(
            &self.state,
            &self.capture,
            self.source_duration,
            self.recording_point_size,
            self.pointer_synthesized,
        );
        self.viewport = Self::build_viewport(
            &self.state,
            &self.capture,
            self.source_size,
            self.source_duration,
            self.recording_point_size,
            &self.pointer,
            self.pointer_synthesized,
        );
        self.compositor = Self::build_compositor(
            &self.state,
            self.source_size,
            self.has_camera,
            &self.viewport,
            &self.pointer,
            self.pointer_synthesized,
            self.crop_editing.is_some(),
            Overlays { keystrokes: &self.keystrokes, subtitles: &self.subtitles, karaoke: &self.karaoke },
        );
        self.playhead = self.playhead.min(self.duration()).max(0.0);
        if self.selected_clip.is_some_and(|id| !self.state.clips.segments.iter().any(|s| s.id == id)) {
            self.selected_clip = None;
        }
        if self.selected_cue.is_some_and(|id| !self.state.zoom_cues.iter().any(|c| c.id == id)) {
            self.selected_cue = None;
        }
        if self.selected_mask.is_some_and(|id| !self.state.masks.iter().any(|m| m.id == id)) {
            self.selected_mask = None;
        }
        if self.selected_shot.as_ref().is_some_and(|id| !self.state.shots_3d.iter().any(|s| s.id == *id)) {
            self.selected_shot = None;
        }
    }

    /// Records an undo step, then applies `edit`.
    pub fn edit(&mut self, edit: impl FnOnce(&mut Snapshot)) {
        self.undo.push(self.state.clone());
        if self.undo.len() > Self::MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
        edit(&mut self.state);
        self.invalidate();
    }

    /// Changes state without an undo step; a slider drag coalesces onto the
    /// step `begin_coalesced` recorded.
    pub fn edit_coalesced(&mut self, edit: impl FnOnce(&mut Snapshot)) {
        edit(&mut self.state);
        self.invalidate();
    }

    pub fn begin_coalesced(&mut self) {
        self.undo.push(self.state.clone());
        if self.undo.len() > Self::MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.state, previous));
        self.invalidate();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.state, next));
        self.invalidate();
        true
    }

    // MARK: - Clips

    pub fn split_at_playhead(&mut self) -> bool {
        let Some((timeline, trailing)) = self.state.clips.split(self.playhead) else { return false };
        self.edit(|s| s.clips = timeline);
        self.selected_clip = Some(trailing);
        true
    }

    pub fn delete_selected_clip(&mut self) -> bool {
        let Some(id) = self.selected_clip else { return false };
        let Some(timeline) = self.state.clips.deleting(id) else { return false };
        let range = self.state.clips.editor_range(id);
        self.edit(|s| s.clips = timeline);
        if let Some((start, _)) = range {
            self.playhead = start.min(self.duration());
        }
        self.selected_clip = None;
        true
    }

    pub fn set_clip_speed(&mut self, id: Uuid, speed: f64, coalesced: bool) {
        let Some(segment) = self.state.clips.segments.iter().find(|s| s.id == id).cloned() else { return };
        let replacement =
            ClipSegment { speed: speed.clamp(ClipSegment::MINIMUM_SPEED, ClipSegment::MAXIMUM_SPEED), ..segment };
        let timeline = self.state.clips.replacing(replacement);
        if coalesced {
            self.edit_coalesced(|s| s.clips = timeline);
        } else {
            self.edit(|s| s.clips = timeline);
        }
    }

    pub fn reset_clips(&mut self) {
        let full = ClipTimeline::full(self.source_duration);
        self.edit(|s| s.clips = full);
    }

    pub fn clip_at_playhead(&self) -> Option<Uuid> {
        self.state.clips.location(self.playhead).map(|l| l.segment_id)
    }

    // MARK: - Zoom

    /// A two-second cue at the playhead's source time, in the free span
    /// between its neighbours (port of `addZoomCue(at:)`).
    pub fn add_zoom_cue_at_playhead(&mut self) -> bool {
        let time = self.source_time(self.playhead);
        let mut start = time;
        let mut end = (time + 2.0).min(self.source_duration);
        for cue in &self.state.zoom_cues {
            if cue.end <= time {
                start = start.max(cue.end);
            } else if cue.start >= time {
                end = end.min(cue.start);
            } else {
                return false; // the playhead is inside a cue
            }
        }
        if end - start < ZoomCue::MINIMUM_DURATION {
            return false;
        }
        let mut cue = ZoomCue::new(start, end).with_zoom(2.0);
        cue.pinned_point = UnitPoint::CENTER;
        if self.capture.travel.is_empty() && self.capture.presses.is_empty() {
            // Without a pointer timeline, only a pinned point can aim the camera.
            cue.anchor_mode = ZoomAnchorMode::PinnedAnchor;
        }
        let id = cue.id;
        self.edit(|s| {
            s.zoom_cues.push(cue);
            s.zoom_cues.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        });
        self.selected_cue = Some(id);
        true
    }

    pub fn remove_zoom_cue(&mut self, id: Uuid) {
        self.edit(|s| s.zoom_cues.retain(|c| c.id != id));
    }

    /// The source-time span a cue may occupy without touching its neighbours.
    pub fn cue_bounds(&self, id: Uuid) -> (f64, f64) {
        let mut lower: f64 = 0.0;
        let mut upper: f64 = self.source_duration;
        if let Some(cue) = self.state.zoom_cues.iter().find(|c| c.id == id) {
            for other in self.state.zoom_cues.iter().filter(|c| c.id != id) {
                if other.end <= cue.start + 1e-9 {
                    lower = lower.max(other.end);
                } else if other.start >= cue.end - 1e-9 {
                    upper = upper.min(other.start);
                }
            }
        }
        (lower, upper)
    }

    pub fn update_zoom_cue(&mut self, cue: ZoomCue, coalesced: bool) {
        let (lower, upper) = self.cue_bounds(cue.id);
        let mut cue = cue;
        cue.start = cue.start.clamp(lower, upper);
        cue.end = cue.end.clamp(lower, upper);
        if cue.end - cue.start < ZoomCue::MINIMUM_DURATION {
            cue.end = (cue.start + ZoomCue::MINIMUM_DURATION).min(upper);
            cue.start = (cue.end - ZoomCue::MINIMUM_DURATION).max(lower);
        }
        cue.zoom = cue.zoom.clamp(1.0, 4.0);
        let apply = |s: &mut Snapshot| {
            if let Some(slot) = s.zoom_cues.iter_mut().find(|c| c.id == cue.id) {
                *slot = cue;
            }
        };
        if coalesced {
            self.edit_coalesced(apply);
        } else {
            self.edit(apply);
        }
    }

    pub fn set_zoom_enabled(&mut self, enabled: bool) {
        if self.state.zoom_enabled != enabled {
            self.edit(|s| s.zoom_enabled = enabled);
        }
    }

    /// Where a cue sits on the edited timeline, if any of it survives the cuts.
    pub fn cue_editor_range(&self, cue: &ZoomCue) -> Option<(f64, f64)> {
        let slices = self.state.clips.slices(cue.start, cue.end);
        let start = slices.first()?.editor_start;
        let end = slices.last()?.editor_end;
        Some((start, end))
    }

    // MARK: - Masks and crop

    pub fn add_mask(&mut self, effect: MaskEffect) {
        let mask = MaskSegment { effect, ..Default::default() };
        let id = mask.id;
        self.edit(|s| s.masks.push(mask));
        self.selected_mask = Some(id);
    }

    pub fn remove_mask(&mut self, id: Uuid) {
        self.edit(|s| s.masks.retain(|m| m.id != id));
    }

    pub fn update_mask(&mut self, mask: MaskSegment, coalesced: bool) {
        let apply = |s: &mut Snapshot| {
            if let Some(slot) = s.masks.iter_mut().find(|m| m.id == mask.id) {
                *slot = mask;
            }
        };
        if coalesced {
            self.edit_coalesced(apply);
        } else {
            self.edit(apply);
        }
    }

    pub fn set_crop(&mut self, crop: UnitRect, coalesced: bool) {
        let crop = crop.sanitized();
        if coalesced {
            self.edit_coalesced(|s| s.crop = crop);
        } else {
            self.edit(|s| s.crop = crop);
        }
    }

    // MARK: - Documents

    /// The document for the current state, keeping fields this build does not edit.
    pub fn document(&self) -> RecordingEditDocument {
        let mut document = self.base_document.clone();
        document.format_version = RecordingEditDocument::CURRENT_FORMAT;
        document.style = (&self.state.style).into();
        document.zoom_enabled = self.state.zoom_enabled;
        document.zoom_cues = self.state.zoom_cues.clone();
        document.set_clips(&self.state.clips);
        document.set_crop(self.state.crop);
        document.set_masks(&self.state.masks);
        document.export_settings = Some(self.state.export_settings.clone());
        document.shows_click_effects = Some(self.state.shows_click_effects);
        document.export_aspect = Some(self.state.export_aspect.raw_value().to_owned());
        document.export_aspect_mode = Some(self.state.export_aspect_mode.raw_value().to_owned());
        document.shows_keystrokes = Some(self.state.shows_keystrokes);
        document.keystroke_placement = Some(self.state.keystroke_placement);
        document.shows_subtitles = Some(self.state.shows_subtitles);
        document.subtitle_vertical_position = Some(self.state.subtitle_style.vertical_position);
        document.subtitle_font_scale = Some(self.state.subtitle_style.font_scale);
        document.subtitle_word_highlight = Some(self.state.subtitle_style.highlights_spoken_word);
        document.subtitle_cues = (!self.state.subtitle_cues.is_empty()).then(|| self.state.subtitle_cues.clone());
        document.subtitle_words = (!self.state.subtitle_words.is_empty()).then(|| self.state.subtitle_words.clone());
        document.shots_3d = (!self.state.shots_3d.is_empty()).then(|| Timeline3D::to_values(&self.state.shots_3d));
        document
    }

    /// Writes the working draft; the explicit save is untouched.
    pub fn write_draft(&self) -> std::io::Result<()> {
        if self.is_dirty() {
            self.session.write_draft_document(&self.document().to_value())
        } else {
            self.session.remove_draft_document();
            Ok(())
        }
    }

    /// Commits the edits to `edit.json`; the deliverable render follows separately.
    pub fn save_document(&mut self) -> std::io::Result<RecordingEditDocument> {
        let document = self.document();
        self.session.write_edit_document(&document.to_value())?;
        self.session.remove_draft_document();
        self.session.update_project_metadata(|m| m.saved_at = Some(chrono::Utc::now()))?;
        self.saved = self.state.clone();
        Ok(document)
    }

    pub fn discard_draft(&self) {
        self.session.remove_draft_document();
    }

    /// Whether the current edits leave the recording as captured, so the
    /// screen master can be delivered without a render.
    pub fn is_unedited(&self) -> bool {
        !self.has_camera
            && !self.pointer_synthesized
            && self.state.clips.is_unedited(self.source_duration)
            && !self.state.style.has_background()
            && (!self.state.zoom_enabled || self.state.zoom_cues.is_empty())
            && self.state.crop.is_unit()
            && self.state.masks.is_empty()
            && self.state.shots_3d.is_empty()
            && self.state.export_aspect.ratio().is_none()
    }

    /// The export of the current edits into `output`, at the project's
    /// export settings (resolution scales the whole canvas).
    pub fn export_request(&self, output: PathBuf) -> ExportRequest {
        let settings = &self.state.export_settings;
        let (mut width, mut height) = self.canvas_size();
        if let Some(target) = settings.resolution.height() {
            let target = f64::from(target);
            if height > target {
                width = (width * target / height / 2.0).round() * 2.0;
                height = target;
            }
        }
        let layout = Self::make_layout(&self.state, self.source_size, self.has_camera, (width, height));
        ExportRequest {
            screen: self.session.screen_path(),
            camera: self.has_camera.then(|| self.session.camera_path()),
            camera_lead_in: self.camera_lead_in,
            style: self.state.style.clone(),
            layout,
            clips: self.state.clips.clone(),
            viewport: self.viewport.clone(),
            crop: self.state.crop,
            masks: self.state.masks.clone(),
            pointer: self.pointer_synthesized.then(|| self.pointer.clone()),
            shows_press_effects: self.state.shows_click_effects,
            motion_blur_samples: settings.speed.motion_blur_samples(),
            shots_3d: self.state.shots_3d.clone(),
            keystrokes: Overlays { keystrokes: &self.keystrokes, subtitles: &self.subtitles, karaoke: &self.karaoke }
                .keystrokes_for(&self.state),
            subtitles: Overlays { keystrokes: &self.keystrokes, subtitles: &self.subtitles, karaoke: &self.karaoke }
                .subtitles_for(&self.state),
            canvas: (width as u32, height as u32),
            frames_per_second: settings.effective_frame_rate().frames_per_second(),
            crf: settings.quality.crf(),
            preset: settings.speed.preset().to_owned(),
            include_audio: !settings.remove_audio,
            audio_bitrate: settings.quality.audio_bitrate(),
            output,
        }
    }

    /// The deliverable's file name for `extension`, named once per project.
    pub fn file_name(
        &self,
        extension: &str,
        prefs: &mut bettershot_core::prefs::Preferences,
    ) -> std::io::Result<String> {
        deliverable::file_name(&self.session, extension, prefs, chrono::Local::now().naive_local())
    }
}

/// The session's caption sources, handed to the compositor as the state
/// shows or hides them.
#[derive(Clone, Copy)]
struct Overlays<'a> {
    keystrokes: &'a KeystrokeCaptionTimeline,
    subtitles: &'a SubtitleTimeline,
    karaoke: &'a KaraokeTimeline,
}

impl Overlays<'_> {
    fn keystrokes_for(&self, state: &Snapshot) -> Option<(KeystrokeCaptionTimeline, KeystrokePlacement)> {
        (state.shows_keystrokes && !self.keystrokes.is_empty())
            .then(|| (self.keystrokes.clone(), state.keystroke_placement))
    }

    fn subtitles_for(&self, state: &Snapshot) -> Option<(SubtitleTimeline, SubtitleBarStyle, Option<KaraokeTimeline>)> {
        (state.shows_subtitles && !self.subtitles.is_empty()).then(|| {
            (
                self.subtitles.clone(),
                state.subtitle_style.clone(),
                (!self.karaoke.is_empty()).then(|| self.karaoke.clone()),
            )
        })
    }
}

/// Port of `RecordingSessionRenderer.ensureDeliverable` for a card's Save:
/// a capture whose cursor must be drawn, or that has edits or a camera, is
/// flattened into the package with its effective document (the defaults
/// when nothing was edited) and stamped, so a later Save reuses it. Blocking;
/// call from a worker. Returns the file to deliver.
pub fn ensure_deliverable(session: &RecordingSession, default_style: StudioStyle) -> Result<PathBuf, String> {
    let document = session.effective_edit_document();
    let manifest = session.load_capture_manifest();
    let synthesized = manifest.as_ref().is_some_and(|m| !m.pointer_in_pixels());
    if !session.has_camera() && !synthesized && document.is_none() {
        return Ok(session.screen_path());
    }
    if let Some(existing) = session.fresh_final_path(document.as_ref()) {
        return Ok(existing);
    }
    let model = StudioModel::load(session.clone(), default_style)?;
    let container = model.state.export_settings.effective_container();
    let temporary = session.directory.join(format!("render-{}.{}", Uuid::new_v4(), container.file_extension()));
    let request = model.export_request(temporary.clone());
    drop(model);
    video::export(&request, &|_| {}, &|| false).map_err(|e| e.to_string())?;
    session.install_final_video(&temporary, document.as_ref()).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        e.to_string()
    })
}
