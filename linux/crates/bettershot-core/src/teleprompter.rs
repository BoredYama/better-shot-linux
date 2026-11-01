//! Port of `TeleprompterScript.swift`: the teleprompter's text model. The
//! script splits into display words, a greedy fuzzy matcher tracks how far
//! a speaker has read from a spoken-word history (when a recognizer feeds
//! one), and a fixed line layout is what the overlay scrolls through. The
//! layout measures words through a closure, so the GTK overlay passes Pango
//! and the tests pass character widths.

/// Display tokens: whitespace-separated words keeping punctuation.
pub fn display_words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

/// Canonical form used to compare a spoken word against a script word:
/// lowercased with everything but letters and digits stripped, so "Hello,"
/// matches "hello" and "it's" matches "its".
pub fn normalize(word: &str) -> String {
    word.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

pub fn normalized_words(text: &str) -> Vec<String> {
    display_words(text).iter().map(|w| normalize(w)).filter(|w| !w.is_empty()).collect()
}

/// Tracks reading progress: given everything heard so far, how many script
/// display words have been spoken. Greedy with a short lookahead, so
/// misrecognized or skipped words don't stall the prompter.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScriptMatcher {
    entries: Vec<(String, usize)>,
}

impl ScriptMatcher {
    /// Script words the recognizer could plausibly skip past in one jump.
    const LOOKAHEAD: usize = 8;

    pub fn new(script: &str) -> Self {
        let entries = display_words(script)
            .iter()
            .enumerate()
            .filter_map(|(index, word)| {
                let normalized = normalize(word);
                (!normalized.is_empty()).then_some((normalized, index))
            })
            .collect();
        Self { entries }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of script display words read so far (the word at index
    /// `count - 1` is the one being spoken).
    pub fn spoken_display_word_count(&self, spoken: &[String]) -> usize {
        let mut position = 0;
        for word in spoken {
            if position >= self.entries.len() {
                break;
            }
            let end = (position + Self::LOOKAHEAD).min(self.entries.len());
            if let Some(found) = (position..end).find(|i| self.entries[*i].0 == *word) {
                position = found + 1;
            }
        }
        if position == 0 {
            return 0;
        }
        self.entries[position - 1].1 + 1
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutWord {
    /// Index into the script's display words (the matcher's counting).
    pub index: usize,
    pub text: String,
}

/// The script wrapped into fixed lines for the overlay, done once up front
/// so the view can scroll line by line and know which line holds the active
/// word. Paragraph breaks are hard line breaks.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScriptLayout {
    pub lines: Vec<Vec<LayoutWord>>,
    line_of_word: Vec<usize>,
}

impl ScriptLayout {
    /// `measure` returns a word's width; `space` is the width of one space.
    pub fn new(script: &str, maximum_line_width: f64, space: f64, measure: impl Fn(&str) -> f64) -> Self {
        let mut lines: Vec<Vec<LayoutWord>> = Vec::new();
        let mut line_of_word = Vec::new();
        let mut current: Vec<LayoutWord> = Vec::new();
        let mut current_width = 0.0;
        let mut word_index = 0;
        for paragraph in script.lines() {
            for word in display_words(paragraph) {
                let width = measure(&word);
                let joined = if current.is_empty() { width } else { current_width + space + width };
                if !current.is_empty() && joined > maximum_line_width {
                    lines.push(std::mem::take(&mut current));
                    current_width = width;
                } else {
                    current_width = joined;
                }
                current.push(LayoutWord { index: word_index, text: word });
                line_of_word.push(lines.len());
                word_index += 1;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0.0;
            }
        }
        Self { lines, line_of_word }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn word_count(&self) -> usize {
        self.line_of_word.len()
    }

    pub fn line_index(&self, word: usize) -> usize {
        if self.line_of_word.is_empty() {
            return 0;
        }
        self.line_of_word[word.min(self.line_of_word.len() - 1)]
    }

    /// Whether this word closes its line: the reader's cue that their eyes
    /// need the next line now, not after the recognizer catches up.
    pub fn is_last_word_in_line(&self, word: usize) -> bool {
        if word >= self.line_of_word.len() {
            return false;
        }
        word + 1 == self.line_of_word.len() || self.line_of_word[word + 1] != self.line_of_word[word]
    }

    /// The first visible line for a spoken count: the line being spoken pins
    /// to the top (one further once its last word is reached), never past
    /// the last window of `visible_lines`.
    pub fn top_line(&self, spoken_word_count: usize, visible_lines: usize) -> usize {
        if spoken_word_count == 0 {
            return 0;
        }
        let active_word = spoken_word_count - 1;
        let mut active_line = self.line_index(active_word);
        if self.is_last_word_in_line(active_word) {
            active_line += 1;
        }
        let maximum_top = self.lines.len().saturating_sub(visible_lines.max(1));
        active_line.min(maximum_top)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizing_strips_punctuation_and_case() {
        assert_eq!(normalize("Hello,"), "hello");
        assert_eq!(normalize("it's"), "its");
        assert_eq!(normalized_words("Hi — there!  ..."), vec!["hi", "there"]);
    }

    #[test]
    fn matcher_skips_misheard_words_within_the_lookahead() {
        let matcher = ScriptMatcher::new("Welcome to the show, everyone. Today we build a thing.");
        assert!(!matcher.is_empty());
        let spoken: Vec<String> = ["welcome", "to", "show"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(matcher.spoken_display_word_count(&spoken), 4, "'the' was skipped; 'show,' is word 4");
        let noise: Vec<String> = ["uh", "um"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(matcher.spoken_display_word_count(&noise), 0);
        let far: Vec<String> = vec!["thing".into()];
        assert_eq!(matcher.spoken_display_word_count(&far), 0, "beyond the lookahead nothing matches");
        let all: Vec<String> = normalized_words("Welcome to the show, everyone. Today we build a thing.");
        assert_eq!(matcher.spoken_display_word_count(&all), 10);
    }

    #[test]
    fn layout_wraps_by_width_and_breaks_paragraphs() {
        // Each character is 1 unit wide, a space 1 unit.
        let layout = ScriptLayout::new("one two three\nfour", 9.0, 1.0, |w| w.len() as f64);
        let lines: Vec<Vec<&str>> = layout.lines.iter().map(|l| l.iter().map(|w| w.text.as_str()).collect()).collect();
        assert_eq!(lines, vec![vec!["one", "two"], vec!["three"], vec!["four"]]);
        assert_eq!(layout.word_count(), 4);
        assert_eq!(layout.line_index(2), 1);
        assert!(layout.is_last_word_in_line(1) && !layout.is_last_word_in_line(0));
        assert_eq!(layout.top_line(0, 2), 0);
        assert_eq!(layout.top_line(1, 2), 0, "mid-line keeps the line on top");
        assert_eq!(layout.top_line(2, 2), 1, "the line's last word scrolls the next line up");
        assert_eq!(layout.top_line(4, 2), 1, "never past the last window");
        assert!(ScriptLayout::new("   ", 10.0, 1.0, |_| 1.0).is_empty());
    }
}
