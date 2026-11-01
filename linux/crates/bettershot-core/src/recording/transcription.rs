//! Port of the cue chunking in `RecordingTranscription.swift`, with a
//! parser for whisper.cpp's JSON output in place of SpeechAnalyzer: the
//! recognizer's timed words become readable subtitle cues on the source
//! timeline, chunked at natural pauses rather than whole sentences.

use super::edit::{SubtitleCue, TranscriptWord};
use serde::Deserialize;
use uuid::Uuid;

/// A full transcription: the word timing (which drives the karaoke
/// highlight) plus the readable cues derived from it (the subtitle bar).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transcript {
    pub words: Vec<TranscriptWord>,
    pub cues: Vec<SubtitleCue>,
}

/// A subtitle should stay readable at a glance, so speech is chunked into
/// short cues at natural pauses rather than whole sentences.
const MAXIMUM_CUE_CHARACTERS: usize = 42;
const MAXIMUM_CUE_DURATION: f64 = 4.0;
const CUE_GAP_THRESHOLD: f64 = 0.9;
const MINIMUM_CUE_DURATION: f64 = 0.8;

/// `RecordingTranscription.makeCues`: words keep the transcript's original
/// spacing, so plain concatenation reconstructs the text exactly.
pub fn make_cues(words: &[TranscriptWord]) -> Vec<SubtitleCue> {
    let mut cues = Vec::new();
    let mut text = String::new();
    let mut start = 0.0;
    let mut end = 0.0;
    let flush = |text: &mut String, cues: &mut Vec<SubtitleCue>, start: f64, end: f64| {
        let trimmed = text.trim().to_owned();
        text.clear();
        if !trimmed.is_empty() {
            cues.push(SubtitleCue {
                id: Uuid::new_v4(),
                start,
                end: end.max(start + MINIMUM_CUE_DURATION),
                text: trimmed,
            });
        }
    };
    for word in words {
        let word_length = word.text.trim().chars().count();
        if !text.is_empty() {
            let breaks_on_length = text.chars().count() + word_length > MAXIMUM_CUE_CHARACTERS;
            let breaks_on_silence = word.start - end > CUE_GAP_THRESHOLD;
            let breaks_on_duration = word.end - start > MAXIMUM_CUE_DURATION;
            if breaks_on_length || breaks_on_silence || breaks_on_duration {
                flush(&mut text, &mut cues, start, end);
            }
        }
        if text.is_empty() {
            start = word.start;
            end = word.end;
        }
        text.push_str(&word.text);
        end = end.max(word.end);
    }
    flush(&mut text, &mut cues, start, end);
    cues
}

#[derive(Deserialize)]
struct WhisperOutput {
    #[serde(default)]
    transcription: Vec<WhisperSegment>,
}

#[derive(Deserialize)]
struct WhisperSegment {
    #[serde(default)]
    offsets: WhisperOffsets,
    #[serde(default)]
    text: String,
    #[serde(default)]
    tokens: Vec<WhisperToken>,
}

#[derive(Deserialize, Default)]
struct WhisperOffsets {
    #[serde(default)]
    from: f64,
    #[serde(default)]
    to: f64,
}

#[derive(Deserialize)]
struct WhisperToken {
    #[serde(default)]
    text: String,
    #[serde(default)]
    offsets: WhisperOffsets,
}

/// Parses whisper.cpp's `--output-json` (or `--output-json-full`) file into
/// timed words. Full output carries token timings, which merge into words at
/// their leading spaces; plain output spreads each segment's words evenly
/// across the segment. Offsets are milliseconds.
pub fn parse_whisper_json(json: &str) -> Result<Transcript, String> {
    let output: WhisperOutput = serde_json::from_str(json).map_err(|e| format!("whisper output: {e}"))?;
    let mut words: Vec<TranscriptWord> = Vec::new();
    for segment in output.transcription {
        let tokens: Vec<&WhisperToken> =
            segment.tokens.iter().filter(|t| !t.text.starts_with('[') && !t.text.trim().is_empty()).collect();
        if tokens.is_empty() {
            let segment_words: Vec<&str> = segment.text.split_whitespace().collect();
            if segment_words.is_empty() {
                continue;
            }
            let (from, to) = (segment.offsets.from / 1000.0, segment.offsets.to / 1000.0);
            let step = (to - from).max(0.0) / segment_words.len() as f64;
            for (index, word) in segment_words.iter().enumerate() {
                let start = from + step * index as f64;
                words.push(TranscriptWord { text: format!("{word} "), start, end: start + step });
            }
            continue;
        }
        // Whisper tokens are sub-word pieces; a token starting with a space
        // begins a new word.
        let mut current: Option<TranscriptWord> = None;
        for token in tokens {
            let starts_word = token.text.starts_with(' ') || current.is_none();
            let (from, to) = (token.offsets.from / 1000.0, token.offsets.to / 1000.0);
            if starts_word {
                if let Some(word) = current.take() {
                    words.push(word);
                }
                current =
                    Some(TranscriptWord { text: token.text.trim_start().to_owned(), start: from, end: to.max(from) });
            } else if let Some(word) = current.as_mut() {
                word.text.push_str(&token.text);
                word.end = word.end.max(to);
            }
        }
        if let Some(word) = current.take() {
            words.push(word);
        }
    }
    // Trailing spaces glue words back together, as the macOS runs do.
    for word in &mut words {
        if !word.text.ends_with(' ') {
            word.text.push(' ');
        }
    }
    let cues = make_cues(&words);
    Ok(Transcript { words, cues })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: f64, end: f64) -> TranscriptWord {
        TranscriptWord { text: format!("{text} "), start, end }
    }

    #[test]
    fn cues_break_on_length_silence_and_duration() {
        let mut words = Vec::new();
        for i in 0..6 {
            words.push(word("word", i as f64 * 0.5, i as f64 * 0.5 + 0.4)); // 0.0–2.9
        }
        words.push(word("later", 5.0, 5.3)); // after a 2.1 s gap
        for i in 0..12 {
            words.push(word("longer", 6.5 + i as f64 * 0.3, 6.7 + i as f64 * 0.3));
            // 12 × 7 chars > 42
        }
        let cues = make_cues(&words);
        assert_eq!(cues[0].text, "word word word word word word");
        assert!((cues[0].start, cues[0].end) == (0.0, 2.9), "{:?}", cues[0]);
        assert_eq!(cues[1].text, "later");
        assert_eq!(cues[1].end, 5.8, "a short cue keeps the minimum duration");
        assert!(cues.len() >= 4, "the long run splits on length: {cues:?}");
        assert!(cues[2..].iter().all(|c| c.text.chars().count() <= 42));
        assert!(make_cues(&[]).is_empty());
        let slow: Vec<TranscriptWord> = (0..5).map(|i| word("w", i as f64 * 1.5, i as f64 * 1.5 + 1.0)).collect();
        let cues = make_cues(&slow);
        assert!(cues.iter().all(|c| c.end - c.start <= 4.5), "cues stay under the duration cap: {cues:?}");
    }

    #[test]
    fn whisper_full_json_merges_tokens_into_words() {
        let json = r#"{"transcription":[{"timestamps":{"from":"00:00:00,000","to":"00:00:01,500"},"offsets":{"from":0,"to":1500},"text":" Hello there, world.","tokens":[
            {"text":"[_BEG_]","offsets":{"from":0,"to":0}},
            {"text":" Hello","offsets":{"from":0,"to":400}},
            {"text":" there","offsets":{"from":500,"to":800}},
            {"text":",","offsets":{"from":800,"to":820}},
            {"text":" wor","offsets":{"from":900,"to":1100}},
            {"text":"ld.","offsets":{"from":1100,"to":1400}}]}]}"#;
        let transcript = parse_whisper_json(json).unwrap();
        let texts: Vec<&str> = transcript.words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, vec!["Hello ", "there, ", "world. "]);
        assert_eq!((transcript.words[1].start, transcript.words[1].end), (0.5, 0.82));
        assert_eq!((transcript.words[2].start, transcript.words[2].end), (0.9, 1.4));
        assert_eq!(transcript.cues.len(), 1);
        assert_eq!(transcript.cues[0].text, "Hello there, world.");
        assert_eq!((transcript.cues[0].start, transcript.cues[0].end), (0.0, 1.4));
    }

    #[test]
    fn whisper_plain_json_spreads_segment_words() {
        let json = r#"{"transcription":[{"offsets":{"from":2000,"to":4000},"text":" four even words here"}]}"#;
        let transcript = parse_whisper_json(json).unwrap();
        assert_eq!(transcript.words.len(), 4);
        assert_eq!((transcript.words[0].start, transcript.words[0].end), (2.0, 2.5));
        assert_eq!((transcript.words[3].start, transcript.words[3].end), (3.5, 4.0));
        assert_eq!(transcript.cues[0].text, "four even words here");
        assert!(parse_whisper_json("nope").is_err());
    }
}
