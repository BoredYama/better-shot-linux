//! The studio's state, a port of the document, undo, timeline, and render
//! parts of `RecordingStudioModel.swift`: the project's style, clips, zoom
//! cues, crop, and masks with undo and redo; the playhead; the frame readers
//! and the shared compositor the preview draws through; and the save and
//! export requests. Everything the window shows comes from here.

use bettershot_core::recording::edit::{
    MaskEffect, MaskSegment, RecordingEditDocument, StudioStyle, UnitRect, VideoCompressionSettings,
};
use bettershot_core::recording::pointer::{self as pointer, PointerTimelineOptions};
use bettershot_core::recording::viewport::{UnitPoint, ZoomAnchorMode};
use bettershot_core::recording::{
    self, deliverable, ClipSegment, ClipTimeline, ContentMode, PointerCaptureFile, PointerTimeline, RecordingSession,
    StudioLayout, ViewportTimeline, ZoomCue,
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
}

pub struct StudioModel {
    pub session: RecordingSession,
    pub source_duration: f64,
    pub source_size: (u32, u32),
    pub has_camera: bool,
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
    pub scissors: bool,
    pub viewport: ViewportTimeline,
    pub pointer: PointerTimeline,
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
        let viewport = Self::build_viewport(&state, &capture);
        let pointer = Self::build_pointer(&state, &capture, source_duration, recording_point_size, pointer_synthesized);
        let compositor =
            Self::build_compositor(&state, source_size, has_camera, &viewport, &pointer, pointer_synthesized);
        Ok(Self {
            session,
            source_duration,
            source_size,
            has_camera,
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
            scissors: false,
            viewport,
            pointer,
            screen,
            camera,
            compositor,
            capture,
        })
    }

    // MARK: - Derived

    /// The output canvas: the source size, or the reframe preset's.
    pub fn canvas_size(&self) -> (f64, f64) {
        let source = (f64::from(self.source_size.0), f64::from(self.source_size.1));
        self.base_document.export_aspect_preset().canvas_size(source)
    }

    fn make_layout(
        state: &Snapshot,
        source_size: (u32, u32),
        has_camera: bool,
        document: &RecordingEditDocument,
    ) -> StudioLayout {
        let source = (f64::from(source_size.0), f64::from(source_size.1));
        let preset = document.export_aspect_preset();
        let canvas = preset.canvas_size(source);
        let reframed = preset.ratio().is_some();
        let fit = reframed
            && document.export_aspect_content_mode() == bettershot_core::recording::edit::ExportAspectContentMode::Fit;
        let aspect = (reframed && source.1 > 0.0).then_some(source.0 / source.1);
        StudioLayout::make(
            canvas,
            &state.style,
            has_camera,
            aspect,
            if fit { ContentMode::Fit } else { ContentMode::Fill },
        )
    }

    fn build_viewport(state: &Snapshot, capture: &PointerCaptureFile) -> ViewportTimeline {
        if state.zoom_enabled {
            ViewportTimeline::build(&state.zoom_cues, capture, &state.clips).cropped(&state.crop)
        } else {
            ViewportTimeline::identity()
        }
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

    fn build_compositor(
        state: &Snapshot,
        source_size: (u32, u32),
        has_camera: bool,
        viewport: &ViewportTimeline,
        pointer: &PointerTimeline,
        synthesized: bool,
    ) -> StudioCompositor {
        let document = RecordingEditDocument::new(&state.style, &state.clips);
        let layout = Self::make_layout(state, source_size, has_camera, &document);
        StudioCompositor::new(
            layout.canvas_size,
            state.style.clone(),
            layout,
            viewport.clone(),
            state.crop,
            state.masks.clone(),
        )
        .with_pointer(synthesized.then(|| pointer.clone()), state.shows_click_effects)
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

    /// Rebuilds the camera path, pointer, and compositor after an edit.
    fn invalidate(&mut self) {
        self.viewport = Self::build_viewport(&self.state, &self.capture);
        self.pointer = Self::build_pointer(
            &self.state,
            &self.capture,
            self.source_duration,
            self.recording_point_size,
            self.pointer_synthesized,
        );
        self.compositor = Self::build_compositor(
            &self.state,
            self.source_size,
            self.has_camera,
            &self.viewport,
            &self.pointer,
            self.pointer_synthesized,
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
            && self.base_document.export_aspect_preset().ratio().is_none()
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
        let layout = {
            let mut state = self.state.clone();
            // The layout is rebuilt for the output canvas; style fractions scale with it.
            state.style = self.state.style.clone();
            let document = &self.base_document;
            let source = (f64::from(self.source_size.0), f64::from(self.source_size.1));
            let preset = document.export_aspect_preset();
            let reframed = preset.ratio().is_some();
            let fit = reframed
                && document.export_aspect_content_mode()
                    == bettershot_core::recording::edit::ExportAspectContentMode::Fit;
            let aspect = (reframed && source.1 > 0.0).then_some(source.0 / source.1);
            let _ = &mut state;
            StudioLayout::make(
                (width, height),
                &self.state.style,
                self.has_camera,
                aspect,
                if fit { ContentMode::Fit } else { ContentMode::Fill },
            )
        };
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
