//! Narration subtitles through whisper.cpp, the optional Linux stand-in for
//! macOS 26's SpeechAnalyzer in `RecordingTranscription.swift`: the
//! recording's audio tracks are mixed down to a 16 kHz mono WAV with
//! GStreamer and handed to the `whisper-cli` command with a ggml model the
//! user chose in Settings; its JSON output becomes timed words and cues
//! (`bettershot-core::recording::transcription`). Nothing is bundled: the
//! command and the model are found on this computer or transcription is
//! reported unavailable.

use crate::media;
use bettershot_core::recording::transcription::{parse_whisper_json, Transcript};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where whisper.cpp is installed, if anywhere.
pub fn command() -> Option<PathBuf> {
    ["whisper-cli", "whisper-cpp", "whisper.cpp", "whisper"].iter().find_map(|name| {
        Command::new(name).arg("--help").stdout(Stdio::null()).stderr(Stdio::null()).status().ok().map(|_| name.into())
    })
}

/// A model from `BETTERSHOT_WHISPER_MODEL`, the chosen path, or the usual
/// model folders (the first `ggml-*.bin` found).
pub fn model_path(chosen: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("BETTERSHOT_WHISPER_MODEL").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(path);
    }
    if let Some(path) = chosen.filter(|p| p.is_file()) {
        return Some(path.to_path_buf());
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let folders = [
        home.join(".local/share/bettershot/models"),
        home.join(".local/share/whisper.cpp/models"),
        home.join(".cache/whisper.cpp"),
        PathBuf::from("/usr/share/whisper.cpp/models"),
        PathBuf::from("/usr/share/whisper-cpp/models"),
    ];
    folders.iter().find_map(|folder| {
        let mut models: Vec<PathBuf> = std::fs::read_dir(folder)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                name.starts_with("ggml-") && name.ends_with(".bin")
            })
            .collect();
        models.sort();
        models.into_iter().next()
    })
}

/// Why transcription cannot run here, if it cannot.
pub fn availability(chosen_model: Option<&Path>) -> Result<(PathBuf, PathBuf), String> {
    let Some(command) = command() else {
        return Err("Install whisper.cpp (the whisper-cli command) to transcribe narration.".into());
    };
    let Some(model) = model_path(chosen_model) else {
        return Err("Choose a whisper.cpp model (a ggml-*.bin file) in Settings › Recording.".into());
    };
    Ok((command, model))
}

/// Transcribes `movie`'s audio with `command` and `model`, working in
/// `scratch`. Blocking; call from a worker.
pub fn transcribe(movie: &Path, command: &Path, model: &Path, scratch: &Path) -> Result<Transcript, String> {
    media::init()?;
    let info = media::probe(movie)?;
    if info.audio_streams == 0 {
        return Err("This recording has no audio to transcribe.".into());
    }
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let wav = scratch.join("narration.wav");
    let mut description = format!(
        "filesrc location=\"{}\" ! qtdemux name=demux audiomixer name=mix ! audioconvert ! audioresample ! audio/x-raw,format=S16LE,rate=16000,channels=1 ! wavenc ! filesink location=\"{}\"",
        media::escape(&movie.to_string_lossy()),
        media::escape(&wav.to_string_lossy())
    );
    for index in 0..info.audio_streams {
        description.push_str(&format!(
            " demux.audio_{index} ! queue ! decodebin ! audioconvert ! audioresample ! audio/x-raw,format=S16LE,rate=16000,channels=1 ! mix."
        ));
    }
    media::run_to_eos(&description, std::time::Duration::from_secs(600))?;
    let base = scratch.join("narration");
    let output = Command::new(command)
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(&wav)
        .args(["-oj", "-ojf", "-np", "-of"])
        .arg(&base)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("whisper could not run: {e}"))?;
    if !output.status.success() {
        return Err(format!("whisper failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let json =
        std::fs::read_to_string(base.with_extension("json")).map_err(|e| format!("whisper wrote no output: {e}"))?;
    let transcript = parse_whisper_json(&json)?;
    if transcript.words.is_empty() {
        return Err("No speech was detected in the narration.".into());
    }
    Ok(transcript)
}
