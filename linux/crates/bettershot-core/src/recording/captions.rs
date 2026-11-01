//! Port of the keystroke caption timeline in `RecordingOverlayEffects.swift`
//! and the subtitle, karaoke, and bar metrics in
//! `RecordingTranscription.swift`: deterministic overlays rebuilt from the
//! project on the source timeline, so the preview and the exporter show the
//! same caption at the same moment.

use super::edit::{KeystrokePlacement, SubtitleBarStyle, SubtitleCue, TranscriptWord};
use super::manifest::RecordingKeystrokeEvent;
use crate::geometry::Rect;

// MARK: - Keystroke captions

/// What the caption shows at a single point in time.
#[derive(Debug, Clone, PartialEq)]
pub struct KeystrokeCaptionFrame {
    pub modifiers: Vec<String>,
    pub key: String,
    pub opacity: f64,
    pub scale: f64,
}

impl KeystrokeCaptionFrame {
    /// The caption text: modifiers, a space, then the key.
    pub fn text(&self) -> (String, String) {
        let modifiers = self.modifiers.join(" ");
        (if modifiers.is_empty() { String::new() } else { format!("{modifiers} ") }, self.key.clone())
    }
}

/// Deterministic caption playback built once from the recorded keystrokes.
/// Consecutive chords replace the caption content in place; a chord with
/// nothing after it fades out on its own.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeystrokeCaptionTimeline {
    events: Vec<RecordingKeystrokeEvent>,
}

impl KeystrokeCaptionTimeline {
    const POP_IN: f64 = 0.16;
    const HOLD: f64 = 1.1;
    const POP_OUT: f64 = 0.3;

    pub fn new(events: &[RecordingKeystrokeEvent]) -> Self {
        let mut events: Vec<RecordingKeystrokeEvent> =
            events.iter().filter(|e| e.time.is_finite() && !e.key.is_empty()).cloned().collect();
        events.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
        Self { events }
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    fn natural_end(event: &RecordingKeystrokeEvent) -> f64 {
        event.time + Self::POP_IN + Self::HOLD + Self::POP_OUT
    }

    pub fn frame(&self, time: f64) -> Option<KeystrokeCaptionFrame> {
        if self.events.is_empty() || !time.is_finite() {
            return None;
        }
        let index = self.events.partition_point(|e| e.time <= time).checked_sub(1)?;
        let event = &self.events[index];
        let natural_end = Self::natural_end(event);
        let next_start = self.events.get(index + 1).map(|e| e.time);
        let end = natural_end.min(next_start.unwrap_or(f64::MAX));
        if time >= end {
            return None;
        }
        // Pop-in is skipped when the previous chord was still on screen: the
        // caption just swaps its content in place.
        let continues = index > 0 && Self::natural_end(&self.events[index - 1]) > event.time;
        let ease = |p: f64| 1.0 - (1.0 - p.clamp(0.0, 1.0)).powi(3);
        let mut opacity = 1.0;
        let mut scale = 1.0;
        if !continues && time - event.time < Self::POP_IN {
            let eased = ease((time - event.time) / Self::POP_IN);
            opacity = eased;
            scale = 0.92 + 0.08 * eased;
        }
        // Fade out only at the natural end; a chord cut short by the next
        // one hands over at full opacity.
        if natural_end <= next_start.unwrap_or(f64::MAX) && time > natural_end - Self::POP_OUT {
            let eased = ease((natural_end - time) / Self::POP_OUT);
            opacity = opacity.min(eased);
            scale = scale.min(0.97 + 0.03 * eased);
        }
        if opacity <= 0.005 {
            return None;
        }
        Some(KeystrokeCaptionFrame { modifiers: event.modifiers.clone(), key: event.key.clone(), opacity, scale })
    }
}

/// Geometry shared by the preview and the exporter: everything derives from
/// the recording card's height, so the caption is stable under zoom and
/// identical at any render resolution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeystrokeCaptionMetrics {
    pub font_size: f64,
    pub padding_horizontal: f64,
    pub padding_vertical: f64,
    pub corner_radius: f64,
    pub margin: f64,
}

impl KeystrokeCaptionMetrics {
    pub const BACKGROUND_ALPHA: f64 = 0.62;
    pub const MODIFIER_ALPHA: f64 = 0.7;

    pub fn new(card_height: f64) -> Self {
        let font_size = (card_height * 0.033).max(11.0);
        Self {
            font_size,
            padding_horizontal: font_size * 0.72,
            padding_vertical: font_size * 0.46,
            corner_radius: font_size * 0.55,
            margin: card_height * 0.045,
        }
    }

    /// Top-left origin of the caption pill inside `card`.
    pub fn pill_origin(&self, pill: (f64, f64), card: Rect, placement: KeystrokePlacement) -> (f64, f64) {
        let x = match placement {
            KeystrokePlacement::TopLeft | KeystrokePlacement::BottomLeft => card.min_x() + self.margin,
            KeystrokePlacement::TopCenter | KeystrokePlacement::BottomCenter => card.mid_x() - pill.0 / 2.0,
            KeystrokePlacement::TopRight | KeystrokePlacement::BottomRight => card.max_x() - self.margin - pill.0,
        };
        let y = if placement.is_top() { card.min_y() + self.margin } else { card.max_y() - self.margin - pill.1 };
        (x, y)
    }
}

// MARK: - Subtitles

/// Deterministic subtitle playback: the cue covering a source time, if any.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SubtitleTimeline {
    cues: Vec<SubtitleCue>,
}

impl SubtitleTimeline {
    pub fn new(cues: &[SubtitleCue]) -> Self {
        let mut cues: Vec<SubtitleCue> =
            cues.iter().filter(|c| c.start.is_finite() && c.end > c.start && !c.text.is_empty()).cloned().collect();
        cues.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        Self { cues }
    }

    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }

    pub fn len(&self) -> usize {
        self.cues.len()
    }

    pub fn cue(&self, time: f64) -> Option<&SubtitleCue> {
        if self.cues.is_empty() || !time.is_finite() {
            return None;
        }
        let index = self.cues.partition_point(|c| c.start <= time).checked_sub(1)?;
        let cue = &self.cues[index];
        (time < cue.end).then_some(cue)
    }

    pub fn text(&self, time: f64) -> Option<&str> {
        self.cue(time).map(|cue| cue.text.as_str())
    }
}

/// Word-level state of the subtitle bar at a moment: the active cue's words
/// plus which one is being spoken.
#[derive(Debug, Clone, PartialEq)]
pub struct KaraokeLine {
    pub words: Vec<String>,
    /// Index of the word being spoken; None between words and after the
    /// cue's last word ends.
    pub active_index: Option<usize>,
    /// How many words have started; spoken words render fully lit even once
    /// the highlight has moved on.
    pub spoken_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct CueLine {
    start: f64,
    end: f64,
    words: Vec<String>,
    timings: Vec<(f64, f64)>,
}

/// Groups timed words into cues the way the cues were chunked from them,
/// but displays the cue's editable text with timings carried over from the
/// recognizer's words.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KaraokeTimeline {
    cues: Vec<CueLine>,
}

impl KaraokeTimeline {
    pub fn new(cues: &[SubtitleCue], words: &[TranscriptWord]) -> Self {
        let mut sorted: Vec<&SubtitleCue> = cues.iter().filter(|c| c.start.is_finite() && c.end > c.start).collect();
        sorted.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        let mut timed_by_cue: Vec<Vec<&TranscriptWord>> = sorted.iter().map(|_| Vec::new()).collect();
        let mut sorted_words: Vec<&TranscriptWord> = words.iter().collect();
        sorted_words.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        let mut cue_index = 0;
        for word in sorted_words {
            while cue_index + 1 < sorted.len() && word.start >= sorted[cue_index + 1].start - 0.001 {
                cue_index += 1;
            }
            let last = timed_by_cue.len().saturating_sub(1);
            if let Some(bucket) = timed_by_cue.get_mut(cue_index.min(last)) {
                bucket.push(word);
            }
        }
        let cues = sorted
            .iter()
            .zip(timed_by_cue)
            .filter_map(|(cue, timed)| {
                if timed.is_empty() {
                    return None;
                }
                let words: Vec<String> = cue.text.split_whitespace().map(str::to_owned).collect();
                if words.is_empty() {
                    return None;
                }
                let timings = (0..words.len())
                    .map(|index| {
                        let timed_index = (index * timed.len() / words.len()).min(timed.len() - 1);
                        (timed[timed_index].start, timed[timed_index].end)
                    })
                    .collect();
                Some(CueLine { start: cue.start, end: cue.end, words, timings })
            })
            .collect();
        Self { cues }
    }

    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }

    /// The karaoke line covering a source time; None when nobody is
    /// speaking or the cue carries no word timings.
    pub fn line(&self, time: f64) -> Option<KaraokeLine> {
        if self.cues.is_empty() || !time.is_finite() {
            return None;
        }
        let index = self.cues.partition_point(|c| c.start <= time).checked_sub(1)?;
        let cue = &self.cues[index];
        if time >= cue.end {
            return None;
        }
        let mut active_index = None;
        let mut spoken_count = 0;
        if let Some(last_started) = cue.timings.iter().rposition(|t| t.0 <= time) {
            spoken_count = last_started + 1;
            // The highlight rides each word until the next one starts; on
            // the cue's last word a small bridge past its end keeps it from
            // flickering off early.
            if last_started < cue.timings.len() - 1 || time <= cue.timings[last_started].1 + 0.25 {
                active_index = Some(last_started);
            }
        }
        Some(KaraokeLine { words: cue.words.clone(), active_index, spoken_count })
    }
}

/// Geometry of the subtitle bar: a rounded black bar with white text,
/// centred horizontally at the style's vertical position, sized off the
/// canvas's smaller dimension so it stays inside portrait canvases.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubtitleBarMetrics {
    pub font_size: f64,
    pub padding_horizontal: f64,
    pub padding_vertical: f64,
    pub corner_radius: f64,
}

impl SubtitleBarMetrics {
    pub const BACKGROUND_ALPHA: f64 = 0.78;
    /// The spoken word in warm yellow; words not yet spoken slightly dimmed.
    pub const KARAOKE_ACCENT: (f64, f64, f64) = (1.0, 0.84, 0.04);
    pub const KARAOKE_UPCOMING_ALPHA: f64 = 0.55;
    /// Widest the bar may grow, as a fraction of the canvas width.
    pub const MAXIMUM_WIDTH_FRACTION: f64 = 0.92;
    /// Wrapped lines allowed before the font starts scaling down.
    pub const MAXIMUM_LINE_COUNT: usize = 3;
    /// Baseline-to-baseline advance as a multiple of ascent + descent.
    pub const LINE_SPACING_FACTOR: f64 = 1.12;

    pub fn new(canvas: (f64, f64), style: &SubtitleBarStyle) -> Self {
        let reference = canvas.0.min(canvas.1);
        let font_size = (reference * 0.032 * style.clamped_font_scale()).max(11.0);
        Self {
            font_size,
            padding_horizontal: font_size * 0.85,
            padding_vertical: font_size * 0.5,
            corner_radius: font_size * 0.6,
        }
    }

    /// Room available for the text line itself on this canvas.
    pub fn maximum_text_width(&self, canvas_width: f64) -> f64 {
        (canvas_width * Self::MAXIMUM_WIDTH_FRACTION - self.padding_horizontal * 2.0).max(24.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn key(time: f64, modifiers: &[&str], key: &str) -> RecordingKeystrokeEvent {
        RecordingKeystrokeEvent {
            time,
            modifiers: modifiers.iter().map(|m| (*m).to_owned()).collect(),
            key: key.into(),
        }
    }

    fn cue(start: f64, end: f64, text: &str) -> SubtitleCue {
        SubtitleCue { id: Uuid::new_v4(), start, end, text: text.into() }
    }

    #[test]
    fn captions_pop_in_hold_swap_and_fade() {
        let timeline =
            KeystrokeCaptionTimeline::new(&[key(1.0, &["Ctrl"], "C"), key(1.5, &["Ctrl"], "V"), key(5.0, &[], "⎋")]);
        assert!(timeline.frame(0.5).is_none());
        let popping = timeline.frame(1.05).unwrap();
        assert!(popping.opacity > 0.0 && popping.opacity < 1.0 && popping.scale < 1.0, "{popping:?}");
        assert_eq!(popping.text(), ("Ctrl ".to_string(), "C".to_string()));
        let held = timeline.frame(1.4).unwrap();
        assert_eq!((held.opacity, held.scale), (1.0, 1.0));
        // The second chord swaps in without a pop (the first was still showing).
        let swapped = timeline.frame(1.51).unwrap();
        assert_eq!((swapped.key.as_str(), swapped.opacity, swapped.scale), ("V", 1.0, 1.0));
        // It fades at its natural end (1.5 + 0.16 + 1.1 + 0.3 = 3.06).
        let fading = timeline.frame(3.0).unwrap();
        assert!(fading.opacity < 0.5, "{fading:?}");
        assert!(timeline.frame(3.1).is_none());
        let escape = timeline.frame(5.2).unwrap();
        assert_eq!(escape.text(), (String::new(), "⎋".to_string()));
    }

    #[test]
    fn caption_pill_placement() {
        let metrics = KeystrokeCaptionMetrics::new(1000.0);
        assert_eq!(metrics.font_size, 33.0);
        let card = Rect::new(100.0, 50.0, 800.0, 1000.0);
        let (x, y) = metrics.pill_origin((200.0, 60.0), card, KeystrokePlacement::BottomCenter);
        assert!((x - 400.0).abs() < 1e-9 && (y - (1050.0 - 45.0 - 60.0)).abs() < 1e-9);
        let (x, y) = metrics.pill_origin((200.0, 60.0), card, KeystrokePlacement::TopRight);
        assert!((x - (900.0 - 45.0 - 200.0)).abs() < 1e-9 && (y - 95.0).abs() < 1e-9);
    }

    #[test]
    fn subtitles_and_karaoke_follow_the_words() {
        let cues = vec![cue(0.0, 2.0, "hello big world"), cue(2.0, 4.0, "second cue"), cue(f64::NAN, 1.0, "bad")];
        let timeline = SubtitleTimeline::new(&cues);
        assert_eq!(timeline.len(), 2);
        assert_eq!(timeline.text(1.0), Some("hello big world"));
        assert_eq!(timeline.text(3.0), Some("second cue"));
        assert_eq!(timeline.text(4.5), None);
        let words = vec![
            TranscriptWord { text: "hello".into(), start: 0.0, end: 0.5 },
            TranscriptWord { text: "big".into(), start: 0.6, end: 0.9 },
            TranscriptWord { text: "world".into(), start: 1.0, end: 1.4 },
            TranscriptWord { text: "second".into(), start: 2.0, end: 2.4 },
            TranscriptWord { text: "cue".into(), start: 2.5, end: 2.9 },
        ];
        let karaoke = KaraokeTimeline::new(&cues, &words);
        let line = karaoke.line(0.7).unwrap();
        assert_eq!(line.words, vec!["hello", "big", "world"]);
        assert_eq!((line.active_index, line.spoken_count), (Some(1), 2));
        // Past the last word's end plus the bridge, the line reads fully spoken.
        let done = karaoke.line(1.9).unwrap();
        assert_eq!((done.active_index, done.spoken_count), (None, 3));
        assert!(karaoke.line(4.5).is_none());
        // Edited text with a different word count still maps timings.
        let edited = vec![cue(0.0, 2.0, "hello there big wide world")];
        let karaoke = KaraokeTimeline::new(&edited, &words[..3]);
        let line = karaoke.line(1.1).unwrap();
        assert_eq!(line.words.len(), 5);
        assert_eq!(line.spoken_count, 5);
        assert!(KaraokeTimeline::new(&cues, &[]).is_empty(), "no word timings, no karaoke");
    }

    #[test]
    fn subtitle_bar_metrics_scale_with_the_smaller_dimension() {
        let style = SubtitleBarStyle::default();
        let wide = SubtitleBarMetrics::new((1920.0, 1080.0), &style);
        let tall = SubtitleBarMetrics::new((1080.0, 1920.0), &style);
        assert!((wide.font_size - 1080.0 * 0.032).abs() < 1e-9);
        assert_eq!(wide.font_size, tall.font_size);
        assert!(wide.maximum_text_width(1920.0) > tall.maximum_text_width(1080.0));
        let bigger = SubtitleBarMetrics::new((1920.0, 1080.0), &SubtitleBarStyle { font_scale: 1.5, ..style });
        assert!(bigger.font_size > wide.font_size);
    }
}
