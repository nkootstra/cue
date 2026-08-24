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
use cue_analysis::Analyzer as _;
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

    // Stage logic emits events; the renderer decides presentation. Core
    // pipeline behavior never depends on terminal output.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let renderer = tokio::spawn(crate::events::run_renderer(rx));

    let result: Result<()> = async {
        for file in &cli.files {
            process_file(file, cli, &config, &tx).await?;
        }
        Ok(())
    }
    .await;

    drop(tx); // close so the renderer can finish
    if let Err(join_err) = renderer.await {
        tracing::warn!(error = %join_err, "event renderer stopped early");
    }

    result.map(|_| 0)
}

async fn process_file(
    file: &str,
    cli: &Cue,
    config: &cue_core::Config,
    events: &tokio::sync::mpsc::UnboundedSender<cue_core::PipelineEvent>,
) -> Result<()> {
    use cue_core::{PipelineEvent, PipelineStage};

    let path = PathBuf::from(file);

    println_line(&format!("Processing {}...", path.display()));

    // ---- Inspect --------------------------------------------------------
    let ffprobe = require_tool("ffprobe", "inspect media files")?;
    let _ = events.send(PipelineEvent::Started(PipelineStage::Inspect));
    let media = cue_media::probe::inspect(&ffprobe, &path).await?;
    print_media_summary(&media);
    let _ = events.send(PipelineEvent::Completed(PipelineStage::Inspect));

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
    let wav_path = stage_dir.join("audio.wav");
    if wav_path.exists() {
        let _ = events.send(PipelineEvent::Cached(PipelineStage::Extract));
    } else {
        let _ = events.send(PipelineEvent::Started(PipelineStage::Extract));
        extract_audio(
            &ffmpeg,
            &path,
            &wav_path,
            &AudioExtractOptions::default(),
        )
        .await?;
        let _ = events.send(PipelineEvent::Completed(PipelineStage::Extract));
    }

    // ---- Transcribe -----------------------------------------------------
    let options = cue_transcription::TranscriptionOptions {
        model: config.transcription.model.clone(),
        language: cli.language.clone(),
    };

    // Cache key: provider + model + language over the extracted audio's
    // bytes. Any change re-transcribes; reruns with the same settings don't.
    let transcript_cache_key = cue_cache::bytes_hash(
        format!(
            "faster-whisper|{}|{}|{}",
            options.model,
            options.language.as_deref().unwrap_or("auto"),
            cue_cache::file_hash(&wav_path)?
        )
        .as_bytes(),
    );
    let transcript_cache =
        cue_cache::JsonCache::new(stage_dir.join("transcription"));

    let transcript = match load_cached(&transcript_cache, &transcript_cache_key) {
        Some(cached) => {
            let _ = events.send(PipelineEvent::Cached(PipelineStage::Transcribe));
            cached
        }
        None => {
            let _ = events.send(PipelineEvent::Started(PipelineStage::Transcribe));
            let transcriber =
                cue_transcription::FasterWhisperTranscriber::resolve(None)?;
            let fresh = transcriber.transcribe(&wav_path, &options).await?;
            store_cached(&transcript_cache, &transcript_cache_key, &fresh);
            let _ = events.send(PipelineEvent::Completed(PipelineStage::Transcribe));
            fresh
        }
    };

    // ---- Normalize (optional; stays local via Ollama) -------------------
    let transcript_hash = cue_cache::bytes_hash(
        serde_json::to_vec(&transcript)
            .map_err(|e| CueError::general("could not hash transcript").because(e.to_string()))?
            .as_slice(),
    );

    // Effective S1 settings participate in the key; when styling/structure/
    // context become configurable they must join this string.
    let normalization_settings_hash = cue_cache::bytes_hash(
        format!("s1|{}|semi-formal|prose|general", config.normalization.provider)
            .as_bytes(),
    );
    let normalized_cache = cue_cache::JsonCache::new(stage_dir.join("normalization"));
    let normalization_key =
        format!("{transcript_hash}-{normalization_settings_hash}");

    let normalized = match load_cached(&normalized_cache, &normalization_key) {
        Some(cached) => {
            let _ = events.send(PipelineEvent::Cached(PipelineStage::Normalize));
            Some(cached)
        }
        None => {
            match cue_normalization::normalize_if_ready(
                &config.normalization.ollama_url,
                &transcript,
            )
            .await
            {
                cue_normalization::NormalizationOutcome::Done(clean) => {
                    store_cached(&normalized_cache, &normalization_key, &clean);
                    let _ =
                        events.send(PipelineEvent::Completed(PipelineStage::Normalize));
                    Some(clean)
                }
                cue_normalization::NormalizationOutcome::Skipped(reason) => {
                    tracing::info!(reason, "normalization skipped");
                    None
                }
            }
        }
    };

    // ---- Analyze (optional; needs a gateway and cleaned text) -----------
    let analysis = match (&config.llm, &normalized) {
        (Some(llm), Some(clean)) => {
            let clean_hash = cue_cache::bytes_hash(
                serde_json::to_vec(clean)
                    .map_err(|e| {
                        CueError::general("could not hash normalized text").because(e.to_string())
                    })?
                    .as_slice(),
            );
            let analysis_key = format!(
                "{}-{}-{}",
                clean_hash,
                llm.model,
                cue_analysis::PROMPT_VERSION
            );
            let analysis_cache =
                cue_cache::JsonCache::new(stage_dir.join("analysis"));

            match load_cached(&analysis_cache, &analysis_key) {
                Some(cached) => {
                    let _ = events.send(PipelineEvent::Cached(PipelineStage::Analyze));
                    Some(cached)
                }
                None => {
                    let _ = events.send(PipelineEvent::Started(PipelineStage::Analyze));
                    let client = cue_llm::ChatClient::new(
                        llm.base_url.clone(),
                        llm.api_key(),
                    );
                    let analyzer =
                        cue_analysis::GatewayAnalyzer::new(client, &llm.model);
                    match analyzer
                        .analyze(&cue_analysis::AnalysisInput::from_normalized(clean))
                        .await
                    {
                        Ok(a) => {
                            store_cached(&analysis_cache, &analysis_key, &a);
                            let _ =
                                events.send(PipelineEvent::Completed(PipelineStage::Analyze));
                            Some(a)
                        }
                        Err(err) => {
                            let _ = events.send(PipelineEvent::Failed {
                                stage: PipelineStage::Analyze,
                                error: err.to_string(),
                            });
                            tracing::warn!(error = %err, "analysis failed");
                            None
                        }
                    }
                }
            }
        }
        (None, _) => {
            tracing::info!("analysis skipped: no LLM gateway configured");
            None
        }
        (Some(_), None) => {
            tracing::info!("analysis skipped: no cleaned text (S1 unavailable)");
            None
        }
    };

    // ---- Render ---------------------------------------------------------
    let _ = events.send(PipelineEvent::Started(PipelineStage::Render));
    let out_dir = output_directory(&path, cli)?;
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| CueError::general(format!("could not create output directory {}", out_dir.display())).because(e.to_string()))?;

    write_json(&out_dir.join("transcript.json"), &transcript)?;
    std::fs::write(out_dir.join("transcript.txt"), transcript.plain_text())
        .map_err(|e| CueError::general("could not write transcript.txt").because(e.to_string()))?;

    if let Some(clean) = &normalized {
        write_json(&out_dir.join("normalized.json"), clean)?;
        std::fs::write(out_dir.join("transcript.clean.txt"), clean.plain_text())
            .map_err(|e| CueError::general("could not write transcript.clean.txt").because(e.to_string()))?;
    }

    if let Some(analysis) = &analysis {
        write_json(&out_dir.join("analysis.json"), analysis)?;
        std::fs::write(out_dir.join("summary.md"), cue_analysis::render_summary(analysis))
            .map_err(|e| CueError::general("could not write summary.md").because(e.to_string()))?;
        std::fs::write(out_dir.join("description.md"), cue_analysis::render_description(analysis))
            .map_err(|e| CueError::general("could not write description.md").because(e.to_string()))?;
    }

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

    let _ = events.send(PipelineEvent::Completed(PipelineStage::Render));
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

/// Read a cache entry, treating corrupt entries as a miss (warned) rather
/// than failing the run.
fn load_cached<T: serde::de::DeserializeOwned>(
    cache: &cue_cache::JsonCache,
    key: &str,
) -> Option<T> {
    match cache.get(key) {
        Ok(Some(value)) => Some(value),
        Ok(None) => None,
        Err(err) => {
            tracing::warn!(error = %err, "ignoring bad cache entry");
            None
        }
    }
}

/// Best-effort store: cache write failures never fail the pipeline.
fn store_cached<T: serde::Serialize>(
    cache: &cue_cache::JsonCache,
    key: &str,
    value: &T,
) {
    if let Err(err) = cache.store(key, value) {
        tracing::warn!(error = %err, "could not store cache entry");
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
