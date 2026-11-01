//! The private screenshot pipeline: staging, saving, and the after-capture
//! decisions. Ports `Sources/Preview/DeckStaging.swift`, the screenshot parts
//! of `ScreenshotFileActions` (`BetterShotPreferences.swift`), and
//! `CaptureOrchestrator.processCapturedImage` / `stageCapture`.
//!
//! Every screenshot starts privately in the deck folder. Copy never exports to
//! the save folder; only explicit Save, capture-and-save, or enabled automatic
//! saving of normal captures writes a deliverable there.

use crate::after_capture::{self, AfterCaptureAction, CaptureType};
use crate::app_prefs::AppPreferences;
use crate::capture_record::CaptureKind;
use crate::file_naming::{self, Kind};
use crate::history::{standardized, HistoryStore};
use crate::imaging::{self, EncodeOptions, ImagingError};
use crate::paths::AppDirs;
use crate::prefs::Preferences;
use crate::shortcuts::Action;
use chrono::Local;
use image::RgbaImage;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Where and how deliverables are written.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveTarget {
    pub directory: PathBuf,
    pub quality: f64,
}

/// Private working images and untouched source companions for capture cards.
pub struct DeckStaging {
    directory: PathBuf,
    saved: HashMap<PathBuf, PathBuf>,
    retained: HashMap<PathBuf, PathBuf>,
}

impl DeckStaging {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self { directory: directory.into(), saved: HashMap::new(), retained: HashMap::new() }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn is_staged(&self, path: &Path) -> bool {
        let directory = standardized(&self.directory);
        let path = standardized(path);
        path != directory && path.starts_with(&directory)
    }

    /// The untouched source beside a staged capture: `name.png` → `name.raw.png`.
    pub fn raw_path(staged: &Path) -> PathBuf {
        staged.with_extension("raw.png")
    }

    /// A folder for one capture, so its staged file carries exactly the
    /// capture's name even when another card has the same one.
    pub fn make_capture_directory(&self) -> io::Result<PathBuf> {
        let folder = self.directory.join(Uuid::new_v4().to_string().to_uppercase());
        fs::create_dir_all(&folder)?;
        Ok(folder)
    }

    pub fn saved_path(&self, path: &Path) -> Option<&Path> {
        self.saved.get(path).map(PathBuf::as_path)
    }

    /// Keeps the editable source and preview inside the library for Edit, Pin,
    /// Share, or drag-out. Returns the library preview, or the input when the
    /// capture is not staged or could not be retained.
    pub fn retain(&mut self, staged: &Path, history: &mut HistoryStore) -> PathBuf {
        if !self.is_staged(staged) {
            return staged.to_path_buf();
        }
        if let Some(retained) = self.retained.get(staged) {
            return retained.clone();
        }
        let extension = staged.extension().and_then(|e| e.to_str()).unwrap_or("png").to_owned();
        let preview_for = move |raw: &Path| raw.with_extension(format!("preview.{extension}"));
        let name = staged.file_name().map(|n| n.to_string_lossy().into_owned());
        let Ok(record) = history.import_capture(
            &Self::raw_path(staged),
            name.as_deref(),
            false,
            CaptureKind::Screenshot,
            Some(&preview_for),
        ) else {
            return staged.to_path_buf();
        };
        let preview = preview_for(&history.path_for_record(&record));
        if fs::copy(staged, &preview).is_err() {
            let _ = history.delete_record(record.id);
            return staged.to_path_buf();
        }
        let _ = history.set_beautified_path(&preview, record.id);
        self.retained.insert(staged.to_path_buf(), preview.clone());
        preview
    }

    /// Copies a staged capture into the save folder and the library. Returns
    /// the saved file; an unstaged input is returned unchanged.
    pub fn promote(&mut self, staged: &Path, history: &mut HistoryStore, target: &SaveTarget) -> io::Result<PathBuf> {
        if !self.is_staged(staged) {
            return Ok(staged.to_path_buf());
        }
        if let Some(saved) = self.saved.get(staged) {
            return Ok(saved.clone());
        }
        let retained = self.retain(staged, history);
        if self.is_staged(&retained) {
            return Err(io::Error::other("The capture could not be added to the library."));
        }
        let name = export_file_name(staged, history, target);
        let destination = save_to_default_location(staged, &name, target).map_err(io::Error::other)?;
        if let Some(id) = history.record_matching(&retained).map(|r| r.id) {
            history.set_beautified_path(&destination, id)?;
        }
        self.saved.insert(staged.to_path_buf(), destination.clone());
        self.retained.insert(staged.to_path_buf(), destination.clone());
        Ok(destination)
    }

    /// Removes a staged capture, its raw source, and its folder. Saved copies stay.
    pub fn discard(&mut self, path: &Path) {
        if !self.is_staged(path) {
            return;
        }
        self.saved.remove(path);
        self.retained.remove(path);
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(Self::raw_path(path));
        if let Some(folder) = path.parent() {
            if standardized(folder) != standardized(&self.directory) {
                let _ = fs::remove_dir(folder);
            }
        }
    }

    /// Clears every staged capture, e.g. at launch.
    pub fn purge(&mut self) {
        self.saved.clear();
        self.retained.clear();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

// MARK: - File actions

/// The name a capture was given when it was taken, without an extension.
pub fn capture_name(path: &Path, history: &HistoryStore) -> String {
    let file_name = history
        .record_matching(path)
        .map(|record| record.display_name().to_owned())
        .unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let stem = Path::new(&file_name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(file_name);
    // `name.raw.png` and `name.preview.png` are the capture's own companions.
    stem.strip_suffix(".raw").or_else(|| stem.strip_suffix(".preview")).map(str::to_owned).unwrap_or(stem)
}

/// The capture's name carrying `path`'s extension.
pub fn export_file_name(path: &Path, history: &HistoryStore, _target: &SaveTarget) -> String {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
    format!("{}.{extension}", capture_name(path, history))
}

/// Writes `source` to a free path for `file_name` in the save folder.
pub fn save_to_default_location(source: &Path, file_name: &str, target: &SaveTarget) -> Result<PathBuf, ImagingError> {
    fs::create_dir_all(&target.directory)?;
    let destination = file_naming::unique_path(file_name, &target.directory);
    imaging::replace_export(source, &destination, target.quality)?;
    Ok(destination)
}

/// Save from the deck or automatic saving. A staged capture is promoted; any
/// other file replaces its existing export or gets a new one.
pub fn save_capture(
    rendered: &Path,
    deck: &mut DeckStaging,
    history: &mut HistoryStore,
    target: &SaveTarget,
) -> io::Result<PathBuf> {
    if deck.is_staged(rendered) {
        let saved = deck.promote(rendered, history, target)?;
        if deck.is_staged(&saved) {
            return Err(io::Error::other("The capture could not be saved."));
        }
        return Ok(saved);
    }
    let record_id = history.record_matching(rendered).map(|r| r.id);
    let destination = match history.annotation_export_path(rendered) {
        Some(existing) => {
            imaging::replace_export(rendered, &existing, target.quality).map_err(io::Error::other)?;
            existing
        }
        None => {
            let name = export_file_name(rendered, history, target);
            save_to_default_location(rendered, &name, target).map_err(io::Error::other)?
        }
    };
    if let Some(id) = record_id {
        history.set_beautified_path(&destination, id)?;
    }
    Ok(destination)
}

/// A private copy for the clipboard or a drag, named after the capture, so
/// paste targets see the right name and the copy survives dismissing the card.
pub fn named_scratch_copy(path: &Path, history: &HistoryStore, folder: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(folder)?;
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
    let destination = folder.join(format!("{}.{extension}", capture_name(path, history)));
    if fs::hard_link(path, &destination).is_err() {
        fs::copy(path, &destination)?;
    }
    Ok(destination)
}

// MARK: - Pipeline

/// What the app should do once a capture is staged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowUp {
    None,
    /// Automatic saving failed; show the card's save error with Retry.
    ShowSaveFailure,
    /// Capture Region & Save: save from the card.
    Save,
    Pin,
    Edit,
}

#[derive(Debug, Clone)]
pub struct ProcessedCapture {
    /// The file the card shows: staged, retained in the library, or saved.
    pub display: PathBuf,
    pub thumbnail: Option<RgbaImage>,
    /// Copy to the clipboard now.
    pub copy: bool,
    pub follow_up: FollowUp,
    /// Automatic saving wrote `display` to the save folder.
    pub saved: bool,
    /// Set when staging failed and the original is shown as-is.
    pub staging_error: Option<String>,
}

/// Mutable state the pipeline works on.
pub struct Pipeline<'a> {
    pub prefs: &'a mut Preferences,
    pub dirs: &'a AppDirs,
    pub history: &'a mut HistoryStore,
    pub deck: &'a mut DeckStaging,
}

/// Normal captures follow the automatic saving setting; explicit Copy, Edit,
/// and Pin shortcuts bypass it.
pub fn allows_automatic_save(action: Action) -> bool {
    matches!(
        action,
        Action::Region
            | Action::Fullscreen
            | Action::Window
            | Action::PreviousRegion
            | Action::TimedRegion
            | Action::ScrollCapture
    )
}

/// The countdown before an action's capture, in seconds.
pub fn capture_delay(action: Action, prefs: &Preferences, dirs: &AppDirs) -> i64 {
    let delay = AppPreferences { prefs, dirs }.self_timer_delay();
    if action == Action::TimedRegion {
        delay.max(3)
    } else {
        delay
    }
}

impl Pipeline<'_> {
    fn target(&self) -> SaveTarget {
        let app = AppPreferences { prefs: self.prefs, dirs: self.dirs };
        SaveTarget { directory: app.save_directory(), quality: app.export_quality() }
    }

    /// Stages `raw` (a PNG the capture produced, which the pipeline takes
    /// ownership of) and decides what happens next.
    pub fn process(&mut self, raw: &Path, action: Action, thumbnail_edge: u32) -> ProcessedCapture {
        let app = AppPreferences { prefs: self.prefs, dirs: self.dirs };
        let options = app.encode_options();
        let keep_in_deck = app.keep_in_deck_until_saved();
        let copy_after_save = app.copy_after_save();
        let open_editor = app.open_editor_after_capture();
        let target = self.target();

        let (staged, thumbnail, staging_error) = match self.stage(raw, options, thumbnail_edge) {
            Ok((staged, thumbnail)) => (staged, Some(thumbnail), None),
            Err((fallback, error)) => (fallback, None, Some(error)),
        };
        let mut display = if keep_in_deck { staged.clone() } else { self.deck.retain(&staged, self.history) };
        if display != staged {
            self.deck.discard(&staged);
        }

        let mut save_failed = false;
        let mut saved = false;
        if allows_automatic_save(action)
            && after_capture::is_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, self.prefs)
        {
            // A staging failure leaves only the original; keep it for retry.
            let result = if staging_error.is_some() {
                Err(io::Error::other("staging failed"))
            } else {
                save_capture(&display, self.deck, self.history, &target)
            };
            match result {
                Ok(path) => {
                    self.deck.discard(&display);
                    display = path;
                    saved = true;
                }
                Err(_) => save_failed = true,
            }
        }

        let copy = action == Action::RegionCopy || (action != Action::RegionSave && copy_after_save);
        let follow_up = if save_failed {
            FollowUp::ShowSaveFailure
        } else if action == Action::RegionSave {
            FollowUp::Save
        } else if action == Action::RegionPin {
            FollowUp::Pin
        } else if action == Action::RegionEdit || (open_editor && action != Action::RegionCopy) {
            FollowUp::Edit
        } else {
            FollowUp::None
        };
        ProcessedCapture { display, thumbnail, copy, follow_up, saved, staging_error }
    }

    /// Names the capture once, writes it in the export format to a private
    /// folder, and moves the untouched source beside it.
    fn stage(
        &mut self,
        raw: &Path,
        options: EncodeOptions,
        thumbnail_edge: u32,
    ) -> Result<(PathBuf, RgbaImage), (PathBuf, String)> {
        let file_name = file_naming::current_file_name(
            options.format.file_extension(),
            Kind::Screenshot,
            Local::now().naive_local(),
            self.prefs,
        )
        .unwrap_or_else(|_| {
            file_naming::file_name(file_naming::DEFAULT_TEMPLATE, options.format.file_extension(), &Default::default())
        });
        let staged = (|| -> Result<(PathBuf, RgbaImage), String> {
            let image = imaging::load_rgba(raw).map_err(|e| e.to_string())?;
            let folder = self.deck.make_capture_directory().map_err(|e| e.to_string())?;
            // Default Look backgrounds are applied here once the shared
            // renderer lands (phase 2); until then the capture is unchanged.
            let staged = imaging::write_new_image(&image, options, &file_name, &folder).map_err(|e| {
                let _ = fs::remove_dir_all(&folder);
                e.to_string()
            })?;
            if let Err(error) = move_file(raw, &DeckStaging::raw_path(&staged)) {
                self.deck.discard(&staged);
                return Err(error.to_string());
            }
            Ok((staged, imaging::thumbnail(&image, thumbnail_edge)))
        })();
        staged.map_err(|error| (self.unstaged_capture(raw, &file_name), error))
    }

    /// When staging fails the original is shown as-is, still carrying the
    /// capture's name, in a private folder of its own.
    fn unstaged_capture(&self, raw: &Path, file_name: &str) -> PathBuf {
        let folder = self.dirs.scratch_folder("Unstaged");
        let extension = raw.extension().and_then(|e| e.to_str()).unwrap_or("png");
        let named = folder.join(file_naming::file_name_with_extension(Path::new(file_name), extension));
        match fs::create_dir_all(&folder).and_then(|_| move_file(raw, &named)) {
            Ok(()) => named,
            Err(_) => raw.to_path_buf(),
        }
    }

    /// Saves a card's capture: explicit Save, Retry after a failed automatic
    /// save, or Capture Region & Save.
    pub fn save(&mut self, display: &Path) -> io::Result<PathBuf> {
        let target = self.target();
        let saved = save_capture(display, self.deck, self.history, &target)?;
        Ok(saved)
    }
}

fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if fs::rename(from, to).is_err() {
        fs::copy(from, to)?;
        fs::remove_file(from)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_prefs::keys;
    use image::Rgba;

    struct Fixture {
        _root: tempfile::TempDir,
        dirs: AppDirs,
        prefs: Preferences,
        history: HistoryStore,
        deck: DeckStaging,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().to_path_buf();
        let dirs = AppDirs::resolve(|key| match key {
            "BETTERSHOT_TESTING" => Some("1".into()),
            "BETTERSHOT_TEST_ROOT" => Some(root_path.clone()),
            _ => None,
        });
        let history = HistoryStore::open(dirs.library(), 100).unwrap();
        let deck = DeckStaging::new(dirs.staging());
        let mut prefs = Preferences::in_memory();
        prefs.set(file_naming::TEMPLATE_KEY, "Shot {counter}").unwrap();
        Fixture { _root: root, dirs, prefs, history, deck }
    }

    fn raw_capture(dirs: &AppDirs) -> PathBuf {
        fs::create_dir_all(&dirs.runtime).unwrap();
        let path = dirs.runtime.join(format!("portal-{}.png", Uuid::new_v4()));
        RgbaImage::from_pixel(40, 20, Rgba([10, 20, 30, 255])).save(&path).unwrap();
        path
    }

    fn files_in(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory)
            .map(|entries| entries.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Mirrors `Tests/DeckStagingCheck.swift` against the production staging.
    #[test]
    fn staging_promote_and_discard() {
        let mut f = fixture();
        let save_dir = f.dirs.default_save_directory();
        let target = SaveTarget { directory: save_dir.clone(), quality: 0.9 };
        let folder = f.deck.make_capture_directory().unwrap();
        let staged = folder.join("bettershot_1.jpg");
        let raw = DeckStaging::raw_path(&staged);
        assert_eq!(raw.file_name().unwrap(), "bettershot_1.raw.png");
        assert!(f.deck.is_staged(&staged));
        assert!(!f.deck.is_staged(&save_dir.join("bettershot_1.jpg")));
        assert!(!f.deck.is_staged(&f.dirs.data.join("deckother.png")), "prefix match needs the separator");

        image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]))
            .save_with_format(&staged, image::ImageFormat::Jpeg)
            .unwrap();
        RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 255])).save(&raw).unwrap();
        let outside = save_dir.join("elsewhere.png");
        assert_eq!(f.deck.promote(&outside, &mut f.history, &target).unwrap(), outside);

        let saved = f.deck.promote(&staged, &mut f.history, &target).unwrap();
        assert_eq!(
            saved,
            save_dir.join("bettershot_1.jpg"),
            "promotion lands in the save folder under the capture's name"
        );
        assert!(saved.exists() && staged.exists(), "the staged copy stays for the deck card");
        assert_eq!(
            f.deck.promote(&staged, &mut f.history, &target).unwrap(),
            saved,
            "promoting twice returns the same file"
        );
        assert_eq!(files_in(&save_dir), ["bettershot_1.jpg"], "promoting twice saves once");
        assert_eq!(f.history.records().len(), 1, "the raw source is in the library");

        f.deck.discard(&staged);
        assert!(!staged.exists() && saved.exists());
        assert!(f.deck.saved_path(&staged).is_none());
        assert!(!folder.exists(), "the capture's folder goes too");
    }

    #[test]
    fn automatic_saving_on() {
        let mut f = fixture();
        after_capture::set_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, true, &mut f.prefs).unwrap();
        let raw = raw_capture(&f.dirs);
        let mut pipeline = Pipeline { prefs: &mut f.prefs, dirs: &f.dirs, history: &mut f.history, deck: &mut f.deck };
        let result = pipeline.process(&raw, Action::Region, 64);
        let save_dir = f.dirs.default_save_directory();
        assert_eq!(result.display, save_dir.join("Shot 1.png"));
        assert!(result.saved);
        assert!(result.copy, "Copy after capture defaults on");
        assert_eq!(result.follow_up, FollowUp::None);
        assert_eq!(result.thumbnail.unwrap().width(), 40, "thumbnails never upscale");
        assert!(!raw.exists(), "the pipeline takes the portal file");
        assert_eq!(files_in(&save_dir), ["Shot 1.png"]);
        assert!(files_in(&f.dirs.staging()).is_empty(), "nothing is left staged");
        let record = &f.history.records()[0];
        assert_eq!(record.display_name(), "Shot 1.png");
        assert_eq!((record.pixel_width, record.pixel_height), (40, 20));
        assert_eq!(
            imaging::load_rgba(&result.display).unwrap().get_pixel(0, 0).0,
            [10, 20, 30, 255],
            "pixels are untouched"
        );
    }

    #[test]
    fn automatic_saving_off_stays_private() {
        let mut f = fixture();
        after_capture::set_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, false, &mut f.prefs).unwrap();
        let raw = raw_capture(&f.dirs);
        let mut pipeline = Pipeline { prefs: &mut f.prefs, dirs: &f.dirs, history: &mut f.history, deck: &mut f.deck };
        let result = pipeline.process(&raw, Action::Region, 64);
        assert!(
            result.display.starts_with(f.dirs.library()),
            "a retained preview in the library: {:?}",
            result.display
        );
        assert_eq!(result.display.file_name().unwrap(), "Shot 1.preview.png");
        assert!(files_in(&f.dirs.default_save_directory()).is_empty(), "nothing reaches the save folder");

        let mut pipeline = Pipeline { prefs: &mut f.prefs, dirs: &f.dirs, history: &mut f.history, deck: &mut f.deck };
        let saved = pipeline.save(&result.display).unwrap();
        assert_eq!(saved.file_name().unwrap(), "Shot 1.png", "an explicit Save uses the capture's name");
        let again = pipeline.save(&saved).unwrap();
        assert_eq!(again, saved, "saving again replaces the same export");
    }

    #[test]
    fn explicit_shortcuts_bypass_automatic_saving() {
        let mut f = fixture();
        after_capture::set_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, true, &mut f.prefs).unwrap();
        for (action, copy, follow_up) in [
            (Action::RegionCopy, true, FollowUp::None),
            (Action::RegionSave, false, FollowUp::Save),
            (Action::RegionPin, true, FollowUp::Pin),
            (Action::RegionEdit, true, FollowUp::Edit),
        ] {
            let raw = raw_capture(&f.dirs);
            let mut pipeline =
                Pipeline { prefs: &mut f.prefs, dirs: &f.dirs, history: &mut f.history, deck: &mut f.deck };
            let result = pipeline.process(&raw, action, 64);
            assert_eq!((result.copy, result.follow_up), (copy, follow_up), "{action:?}");
            assert!(
                !result.display.starts_with(f.dirs.default_save_directory()),
                "{action:?} bypasses automatic saving"
            );
        }
        assert!(files_in(&f.dirs.default_save_directory()).is_empty());
    }

    #[test]
    fn keep_in_deck_and_failed_saves_are_retained_for_retry() {
        let mut f = fixture();
        f.prefs.set(keys::KEEP_IN_DECK_UNTIL_SAVED, true).unwrap();
        after_capture::set_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, false, &mut f.prefs).unwrap();
        let raw = raw_capture(&f.dirs);
        let mut pipeline = Pipeline { prefs: &mut f.prefs, dirs: &f.dirs, history: &mut f.history, deck: &mut f.deck };
        let kept = pipeline.process(&raw, Action::Region, 64);
        assert!(f.deck.is_staged(&kept.display), "keep in deck shows the staged file");
        assert!(f.history.records().is_empty(), "nothing enters the library until it is needed");

        // A save folder that is a file cannot be written.
        let blocked = f.dirs.data.join("blocked");
        fs::create_dir_all(&f.dirs.data).unwrap();
        fs::write(&blocked, b"").unwrap();
        f.prefs.set(keys::SAVE_DIRECTORY, blocked.to_string_lossy().as_ref()).unwrap();
        f.prefs.set(keys::KEEP_IN_DECK_UNTIL_SAVED, false).unwrap();
        after_capture::set_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, true, &mut f.prefs).unwrap();
        let raw = raw_capture(&f.dirs);
        let mut pipeline = Pipeline { prefs: &mut f.prefs, dirs: &f.dirs, history: &mut f.history, deck: &mut f.deck };
        let failed = pipeline.process(&raw, Action::Region, 64);
        assert_eq!(failed.follow_up, FollowUp::ShowSaveFailure);
        assert!(!failed.saved);
        assert!(failed.display.exists(), "the capture is kept for retry");

        let fixed = f.dirs.data.join("fixed");
        f.prefs.set(keys::SAVE_DIRECTORY, fixed.to_string_lossy().as_ref()).unwrap();
        let mut pipeline = Pipeline { prefs: &mut f.prefs, dirs: &f.dirs, history: &mut f.history, deck: &mut f.deck };
        let retried = pipeline.save(&failed.display).unwrap();
        assert!(retried.starts_with(&fixed));
    }

    #[test]
    fn unreadable_captures_are_still_shown() {
        let mut f = fixture();
        fs::create_dir_all(&f.dirs.runtime).unwrap();
        let raw = f.dirs.runtime.join("broken.png");
        fs::write(&raw, b"not a png").unwrap();
        let mut pipeline = Pipeline { prefs: &mut f.prefs, dirs: &f.dirs, history: &mut f.history, deck: &mut f.deck };
        let result = pipeline.process(&raw, Action::Region, 64);
        assert!(result.staging_error.is_some());
        assert_eq!(result.display.file_name().unwrap(), "Shot 1.png", "it still carries the capture's name");
        assert!(result.display.exists());
    }

    #[test]
    fn scratch_copies_carry_the_capture_name() {
        let mut f = fixture();
        let raw = raw_capture(&f.dirs);
        after_capture::set_enabled(AfterCaptureAction::Save, CaptureType::Screenshot, false, &mut f.prefs).unwrap();
        let mut pipeline = Pipeline { prefs: &mut f.prefs, dirs: &f.dirs, history: &mut f.history, deck: &mut f.deck };
        let result = pipeline.process(&raw, Action::Region, 64);
        let copy = named_scratch_copy(&result.display, &f.history, &f.dirs.scratch_folder("Clipboard")).unwrap();
        assert_eq!(copy.file_name().unwrap(), "Shot 1.png");
        assert!(capture_delay(Action::TimedRegion, &f.prefs, &f.dirs) >= 3);
        assert_eq!(capture_delay(Action::Region, &f.prefs, &f.dirs), 0);
    }
}
