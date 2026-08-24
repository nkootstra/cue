//! The default command: process media files through the local pipeline.
//!
//! Stages in this milestone:
//!
//! ```text
//! inspect -> extract -> transcribe -> write transcript outputs
//! ```
//!
//! Subtitles, normalization, and analysis come online in later phases.

use std::path::{Path, PathBuf};

use cue_core::config::{load_user_config, resolve, PartialConfig};
use cue_core::media::Media;
use cue_core::{CueError, Result};
use cue_media::extract::{extract_audio, AudioExtractOptions};
use cue_transcription::Transcriber;

use crate::cli::Cue;
use crate::render::{human_duration, println_line};

pub async fn run(cli: &Cue) -> i32 {
    match run_inner(cli).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

async fn run_inner(cli: &Cue) -> Result<i32> {
    if cli.files.is_empty() {
        print_usage_hint();
        return Ok(0);
    }

    let user = load_user_config()?;
    let config = resolve(&[&PartialConfig::default(), &user]);
    tracing::debug!(?config, "resolved configuration");

    for file in &cli.files {
        process_file(file, cli, &config).await?;
    }
    Ok(0)
}

async fn process_file(file: &str, cli: &Cue, config: &cue_core::Config) -> Result<()> {
    let path = PathBuf::from(file);

    println_line(&format!("Processing {}...", path.display()));

    // ---- Inspect --------------------------------------------------------
    let ffprobe = require_tool("ffprobe", "inspect media files")?;
    println_line("  [1/4] inspecting");
    let media = cue_media::probe::inspect(&ffprobe, &path).await?;
    print_media_summary(&media);

    if !media.has_audio() {
        return Err(CueError::new(
            cue_core::PipelineStage::Extract,
            format!("{} contains no audio stream", path.display()),
        )
        .remedy("cue can only process files that contain audio"));
    }

    // Content cache layout keyed by the media bytes themselves.
    let media_hash = cue_cache::file_hash(&path)?;
    let cache_root = cue_cache::cache_dir().ok_or_else(|| {
        CueError::general("could not determine a cache directory")
            .remedy("set CUE_CACHE_DIR to a writable directory")
    })?;
    let stage_dir = cue_cache::media_dir(&cache_root, &media_hash);
    std::fs::create_dir_all(&stage_dir)
        .map_err(|e| CueError::general("could not create cache directory").because(e.to_string()))?;

    // Persist inspection so later stages (and reruns) can reuse it.
    write_json(&stage_dir.join("media.json"), &media)?;

    // ---- Extract --------------------------------------------------------
    let ffmpeg = require_tool("ffmpeg", "extract audio")?;
    println_line("  [2/4] extracting audio");
    let wav_path = stage_dir.join("audio.wav");
    if !wav_path.exists() {
        extract_audio(
            &ffmpeg,
            &path,
            &wav_path,
            &AudioExtractOptions::default(),
        )
        .await?;
    } else {
        println_line("         cached");
    }

    // ---- Transcribe -----------------------------------------------------
    println_line("  [3/4] transcribing");
    let transcriber = cue_transcription::FasterWhisperTranscriber::resolve(None)?;
    let options = cue_transcription::TranscriptionOptions {
        model: config.transcription.model.clone(),
        language: cli.language.clone(),
    };
    let transcript = transcriber.transcribe(&wav_path, &options).await?;

    // ---- Render ---------------------------------------------------------
    println_line("  [4/4] writing outputs");
    let out_dir = output_directory(&path, cli)?;
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| CueError::general(format!("could not create output directory {}", out_dir.display())).because(e.to_string()))?;

    write_json(&out_dir.join("transcript.json"), &transcript)?;
    std::fs::write(out_dir.join("transcript.txt"), transcript.plain_text())
        .map_err(|e| CueError::general("could not write transcript.txt").because(e.to_string()))?;

    // Subtitles derive from the canonical transcript, never from cleaned
    // text.
    let policy = cue_subtitles::SubtitlePolicy {
        max_lines: config.subtitles.max_lines,
        max_chars_per_line: config.subtitles.max_chars_per_line,
        max_duration_ms: config.subtitles.max_duration_ms,
        max_chars_per_second: config.subtitles.max_chars_per_second,
    };
    let cues = cue_subtitles::build_cues(&transcript, &policy);
    for format in &config.subtitles.formats {
        let path = out_dir.join(format!("subtitles.{}", format.extension()));
        let content = match format {
            cue_core::config::SubtitleFormat::Srt => {
                cue_subtitles::render_srt(&cues)
            }
            cue_core::config::SubtitleFormat::Vtt => {
                cue_subtitles::render_vtt(&cues)
            }
        };
        std::fs::write(&path, content).map_err(|e| {
            CueError::general(format!("could not write {}", path.display())).because(e.to_string())
        })?;
    }

    println_line(&format!(
        "\nDone. Transcript and subtitles written to {}/",
        out_dir.display()
    ));
    Ok(())
}

fn require_tool(binary: &str, purpose: &str) -> Result<PathBuf> {
    cue_media::tools::find_on_path(binary).ok_or_else(|| {
        CueError::general(format!(
            "{} is required to {purpose}",
            capitalize(binary)
        ))
        .because(format!("{binary} was not found on PATH"))
        .remedy("install FFmpeg and verify with `cue doctor`")
    })
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `<stem>.cue/` next to the input, or `--output <dir>` when given.
fn output_directory(input: &Path, cli: &Cue) -> Result<PathBuf> {
    if let Some(dir) = &cli.output {
        return Ok(PathBuf::from(dir));
    }
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "output".to_string());
    match input.parent() {
        Some(parent) => Ok(parent.join(format!("{stem}.cue"))),
        None => Ok(PathBuf::from(format!("{stem}.cue"))),
    }
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| CueError::general("serialization failed").because(e.to_string()))?;
    std::fs::write(path, json + "\n")
        .map_err(|e| CueError::general(format!("could not write {}", path.display())).because(e.to_string()))
}

fn print_usage_hint() {
    println_line("cue processes local video and audio files.");
    println_line("\nUsage:");
    println_line("    cue <file>...");
    println_line("\nOther commands:");
    println_line("    doctor    Check the local environment");
    println_line("    models    Manage transcription and normalization models");
    println_line("    config    Show resolved configuration");
    println_line("    cache     Manage the processing cache");
}

fn print_media_summary(media: &Media) {
    println_line(&format!("  format:     {}", media.format));
    println_line(&format!(
        "  duration:   {}",
        human_duration(media.duration_ms)
    ));

    if media.has_video() {
        for stream in &media.video_streams {
            println_line(&format!(
                "  video:      #{} {} ({}x{} @ {})",
                stream.index, stream.codec, stream.width, stream.height, stream.frame_rate
            ));
        }
    } else {
        println_line("  video:      none");
    }

    for stream in &media.audio_streams {
        println_line(&format!(
            "  audio:      #{} {} ({} Hz, {} ch)",
            stream.index, stream.codec, stream.sample_rate_hz, stream.channels
        ));
    }
}
