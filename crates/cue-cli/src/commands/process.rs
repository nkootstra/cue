//! The default command: process media files.
//!
//! In this milestone the pipeline covers the Inspect stage; transcription,
//! subtitles, normalization, and analysis come online in later phases.

use std::path::Path;

use cue_core::config::{load_user_config, resolve, PartialConfig};
use cue_core::media::Media;
use cue_core::CueError;
use cue_media::tools::find_on_path;

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

async fn run_inner(cli: &Cue) -> cue_core::Result<i32> {
    if cli.files.is_empty() {
        print_usage_hint();
        return Ok(0);
    }

    let user = load_user_config()?;
    let _config = resolve(&[&PartialConfig::default(), &user]);

    let ffprobe = require_ffprobe()?;

    for file in &cli.files {
        let path = Path::new(file);
        println_line(&format!("\nInspecting {}...", path.display()));
        let media = cue_media::probe::inspect(&ffprobe, path).await?;
        print_media_summary(&media);
    }

    println_line("\nInspection complete.");
    println_line("Transcription and subtitle stages are not implemented yet.");
    Ok(0)
}

fn require_ffprobe() -> cue_core::Result<std::path::PathBuf> {
    find_on_path("ffprobe").ok_or_else(|| {
        CueError::general("FFprobe is required to inspect media files")
            .because("ffprobe was not found on PATH")
            .remedy(
                "install FFmpeg (which provides ffprobe), then verify with `cue doctor`",
            )
    })
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

    if media.has_audio() {
        for stream in &media.audio_streams {
            println_line(&format!(
                "  audio:      #{} {} ({} Hz, {} ch)",
                stream.index, stream.codec, stream.sample_rate_hz, stream.channels
            ));
        }
    } else {
        println_line("  audio:      none — this file cannot be transcribed");
    }
}
