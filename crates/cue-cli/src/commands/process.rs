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

use cue_analysis::Analyzer as _;
use cue_core::media::Media;
use cue_core::{CueError, Result};
use cue_media::extract::extract_audio;
use cue_transcription::Transcriber;

use crate::cli::Cue;
use crate::render::{human_duration, println_line};

#[derive(serde::Serialize)]
struct TranscriptionCacheKey<'a> {
    version: u8,
    provider: &'static str,
    model: &'a str,
    language: Option<&'a str>,
    audio_hash: &'a str,
}

#[derive(serde::Serialize)]
struct NormalizationCacheKey<'a> {
    version: u8,
    transcript_hash: &'a str,
    provider: &'a str,
    styling: &'a str,
    structure: &'a str,
    context: &'a str,
}

#[derive(serde::Serialize)]
struct AnalysisCacheKey<'a> {
    version: u8,
    normalized_hash: &'a str,
    language: &'a str,
    model: &'a str,
    prompt_version: u32,
}

pub async fn run(cli: &Cue, config: &cue_core::Config) -> i32 {
    run_stopped(cli, &cli.files, None, config).await
}

/// Process files, optionally stopping after a given stage.
///
/// `cue transcribe` uses this to produce transcripts without subtitles,
/// normalization, or analysis.
pub async fn run_stopped(
    cli: &Cue,
    files: &[String],
    stop_after: Option<cue_core::PipelineStage>,
    config: &cue_core::Config,
) -> i32 {
    match run_inner(files, stop_after, cli, config).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

async fn run_inner(
    files: &[String],
    stop_after: Option<cue_core::PipelineStage>,
    cli: &Cue,
    config: &cue_core::Config,
) -> Result<i32> {
    if files.is_empty() {
        print_usage_hint();
        return Ok(0);
    }

    // Stage logic emits events; the renderer decides presentation. Core
    // pipeline behavior never depends on terminal output.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let renderer = tokio::spawn(crate::events::run_renderer(rx));

    let result: Result<()> = async {
        let mut s1_readiness: Option<std::result::Result<bool, String>> = None;
        for file in files {
            process_file(file, cli, config, &tx, stop_after, &mut s1_readiness).await?;
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
    stop_after: Option<cue_core::PipelineStage>,
    s1_readiness: &mut Option<std::result::Result<bool, String>>,
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
    std::fs::create_dir_all(&stage_dir).map_err(|e| {
        CueError::general("could not create cache directory").because(e.to_string())
    })?;

    // Persist inspection so later stages (and reruns) can reuse it.
    write_json(&stage_dir.join("media.json"), &media)?;

    // ---- Extract --------------------------------------------------------
    let ffmpeg = require_tool("ffmpeg", "extract audio")?;
    let wav_path = stage_dir.join("audio.wav");
    if wav_path.exists() {
        let _ = events.send(PipelineEvent::Cached(PipelineStage::Extract));
    } else {
        let _ = events.send(PipelineEvent::Started(PipelineStage::Extract));
        extract_audio(&ffmpeg, &path, &wav_path).await?;
        let _ = events.send(PipelineEvent::Completed(PipelineStage::Extract));
    }

    // ---- Transcribe -----------------------------------------------------
    let options = cue_transcription::TranscriptionOptions {
        model: config.transcription.model.clone(),
        language: cli.language.clone(),
    };

    // Any output-affecting input participates in the logical key. JsonCache
    // hashes its structured representation into a filesystem-safe identity.
    let audio_hash = cue_cache::file_hash(&wav_path)?;
    let transcript_cache_key = TranscriptionCacheKey {
        version: 1,
        provider: "faster-whisper",
        model: &options.model,
        language: options.language.as_deref(),
        audio_hash: &audio_hash,
    };
    let transcript_cache = cue_cache::JsonCache::new(stage_dir.join("transcription"));
    let transcript = match load_cached(&transcript_cache, &transcript_cache_key) {
        Some(cached) => {
            let _ = events.send(PipelineEvent::Cached(PipelineStage::Transcribe));
            cached
        }
        None => {
            let _ = events.send(PipelineEvent::Started(PipelineStage::Transcribe));
            let transcriber = cue_transcription::FasterWhisperTranscriber::resolve(None)?;
            let fresh = transcriber
                .transcribe_with_progress(&wav_path, &options, Some(events.clone()))
                .await?;
            store_cached(&transcript_cache, &transcript_cache_key, &fresh);
            let _ = events.send(PipelineEvent::Completed(PipelineStage::Transcribe));
            fresh
        }
    };

    // `cue transcribe` stops here: canonical transcript only.
    if stop_after == Some(PipelineStage::Transcribe) {
        let _ = events.send(PipelineEvent::Started(PipelineStage::Render));
        let out_dir = output_directory(&path, cli)?;
        std::fs::create_dir_all(&out_dir).map_err(|e| {
            CueError::general(format!(
                "could not create output directory {}",
                out_dir.display()
            ))
            .because(e.to_string())
        })?;
        write_json(&out_dir.join("transcript.json"), &transcript)?;
        std::fs::write(out_dir.join("transcript.txt"), transcript.plain_text()).map_err(|e| {
            CueError::general("could not write transcript.txt").because(e.to_string())
        })?;
        let _ = events.send(PipelineEvent::Completed(PipelineStage::Render));
        println_line(&format!(
            "\nDone. Transcript written to {}/",
            out_dir.display()
        ));
        return Ok(());
    }

    // ---- Normalize (optional; stays local via Ollama) -------------------
    let transcript_hash = cue_cache::bytes_hash(
        serde_json::to_vec(&transcript)
            .map_err(|e| CueError::general("could not hash transcript").because(e.to_string()))?
            .as_slice(),
    );

    // Effective S1 settings participate in the key: changing the control
    // line re-normalizes.
    let s1_settings = cue_normalization::S1Settings {
        styling: config.normalization.styling.clone(),
        structure: config.normalization.structure.clone(),
        context: config.normalization.context.clone(),
    };
    let normalized_cache = cue_cache::JsonCache::new(stage_dir.join("normalization"));
    let normalization_key = NormalizationCacheKey {
        version: 1,
        transcript_hash: &transcript_hash,
        provider: &config.normalization.provider,
        styling: &config.normalization.styling,
        structure: &config.normalization.structure,
        context: &config.normalization.context,
    };

    let normalized = match load_cached(&normalized_cache, &normalization_key) {
        Some(cached) => {
            let _ = events.send(PipelineEvent::Cached(PipelineStage::Normalize));
            Some(cached)
        }
        None => {
            if s1_readiness.is_none() {
                let admin = cue_llm::OllamaAdmin::new(&config.normalization.ollama_url);
                *s1_readiness = Some(
                    cue_normalization::s1_ready(&admin)
                        .await
                        .map_err(|err| err.to_string()),
                );
            }
            let outcome = match s1_readiness.as_ref().expect("readiness set above") {
                Ok(true) => match cue_normalization::normalize_s1(
                    &config.normalization.ollama_url,
                    s1_settings,
                    &transcript,
                )
                .await
                {
                    Ok(clean) => cue_normalization::NormalizationOutcome::Done(clean),
                    Err(err) => cue_normalization::NormalizationOutcome::Skipped(err.to_string()),
                },
                Ok(false) => cue_normalization::NormalizationOutcome::Skipped(format!(
                    "model \"{}\" not found in Ollama",
                    cue_normalization::S1_MODEL_NAME
                )),
                Err(reason) => cue_normalization::NormalizationOutcome::Skipped(format!(
                    "could not determine S1 readiness: {reason}"
                )),
            };
            match outcome {
                cue_normalization::NormalizationOutcome::Done(clean) => {
                    store_cached(&normalized_cache, &normalization_key, &clean);
                    let _ = events.send(PipelineEvent::Completed(PipelineStage::Normalize));
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
            let analysis_key = AnalysisCacheKey {
                version: 2,
                normalized_hash: &clean_hash,
                language: &transcript.language,
                model: &llm.model,
                prompt_version: cue_analysis::PROMPT_VERSION,
            };
            let analysis_cache = cue_cache::JsonCache::new(stage_dir.join("analysis"));

            match load_cached(&analysis_cache, &analysis_key) {
                Some(cached) => {
                    let _ = events.send(PipelineEvent::Cached(PipelineStage::Analyze));
                    Some(cached)
                }
                None => {
                    let _ = events.send(PipelineEvent::Started(PipelineStage::Analyze));
                    let client = cue_llm::ChatClient::new(llm.base_url.clone(), llm.api_key());
                    let analyzer = cue_analysis::GatewayAnalyzer::new(client, &llm.model);
                    match analyzer
                        .analyze(&cue_analysis::AnalysisInput::from_normalized(
                            &transcript.language,
                            clean,
                        ))
                        .await
                    {
                        Ok(a) => {
                            store_cached(&analysis_cache, &analysis_key, &a);
                            let _ = events.send(PipelineEvent::Completed(PipelineStage::Analyze));
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
    std::fs::create_dir_all(&out_dir).map_err(|e| {
        CueError::general(format!(
            "could not create output directory {}",
            out_dir.display()
        ))
        .because(e.to_string())
    })?;

    write_json(&out_dir.join("transcript.json"), &transcript)?;
    std::fs::write(out_dir.join("transcript.txt"), transcript.plain_text())
        .map_err(|e| CueError::general("could not write transcript.txt").because(e.to_string()))?;

    if let Some(clean) = &normalized {
        write_json(&out_dir.join("normalized.json"), clean)?;
        std::fs::write(out_dir.join("transcript.clean.txt"), clean.plain_text()).map_err(|e| {
            CueError::general("could not write transcript.clean.txt").because(e.to_string())
        })?;
    }

    if let Some(analysis) = &analysis {
        write_json(&out_dir.join("analysis.json"), analysis)?;
        std::fs::write(
            out_dir.join("summary.md"),
            cue_analysis::render_summary(analysis),
        )
        .map_err(|e| CueError::general("could not write summary.md").because(e.to_string()))?;
        std::fs::write(
            out_dir.join("description.md"),
            cue_analysis::render_description(analysis),
        )
        .map_err(|e| CueError::general("could not write description.md").because(e.to_string()))?;
    }

    // Subtitles derive from the canonical transcript, never from cleaned
    // text.
    let policy = cue_subtitles::SubtitlePolicy {
        max_lines: config.subtitles.max_lines,
        max_chars_per_line: config.subtitles.max_chars_per_line,
        max_duration_ms: config.subtitles.max_duration_ms,
    };
    let cues = cue_subtitles::build_cues(&transcript, &policy);
    for format in &config.subtitles.formats {
        let path = out_dir.join(format!("subtitles.{}", format.extension()));
        let content = match format {
            cue_core::config::SubtitleFormat::Srt => cue_subtitles::render_srt(&cues),
            cue_core::config::SubtitleFormat::Vtt => cue_subtitles::render_vtt(&cues),
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
        CueError::general(format!("{} is required to {purpose}", capitalize(binary)))
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
    key: &impl serde::Serialize,
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
    key: &impl serde::Serialize,
    value: &T,
) {
    if let Err(err) = cache.store(key, value) {
        tracing::warn!(error = %err, "could not store cache entry");
    }
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| CueError::general("serialization failed").because(e.to_string()))?;
    std::fs::write(path, json + "\n").map_err(|e| {
        CueError::general(format!("could not write {}", path.display())).because(e.to_string())
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

    for stream in &media.audio_streams {
        println_line(&format!(
            "  audio:      #{} {} ({} Hz, {} ch)",
            stream.index, stream.codec, stream.sample_rate_hz, stream.channels
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_cache_identity_includes_canonical_language() {
        let english = AnalysisCacheKey {
            version: 2,
            normalized_hash: "same-normalized-text",
            language: "en",
            model: "model",
            prompt_version: cue_analysis::PROMPT_VERSION,
        };
        let dutch = AnalysisCacheKey {
            language: "nl",
            ..english
        };

        assert_ne!(
            serde_json::to_vec(&english).unwrap(),
            serde_json::to_vec(&dutch).unwrap()
        );
    }
}
