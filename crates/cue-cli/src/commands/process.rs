//! The default command: process media files through the local pipeline.
//!
//! Stages in this milestone:
//!
//! ```text
//! inspect -> extract -> transcribe -> write transcript outputs
//! ```
//!
//! Subtitles, normalization, and analysis come online in later phases.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cue_analysis::Analyzer as _;
use cue_core::media::Media;
use cue_core::{CueError, Result};
use cue_media::extract::extract_audio;
use cue_transcription::Transcriber;

use crate::cli::Cue;
use crate::commands::inputs::{ResolvedInput, resolve_inputs};
use crate::corrections::{CorrectionPlan, CorrectionScope};
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
    endpoint: &'a str,
    styling: &'a str,
    structure: &'a str,
    context: &'a str,
}

const NORMALIZATION_CACHE_VERSION: u8 = 3;

#[derive(serde::Serialize)]
struct AnalysisCacheKey<'a> {
    version: u8,
    normalized_hash: &'a str,
    language: &'a str,
    endpoint: &'a str,
    model: &'a str,
    prompt_version: u32,
}

fn normalized_endpoint(endpoint: &str) -> &str {
    endpoint.trim_end_matches('/')
}

fn remove_stale_artifacts(out_dir: &Path, names: &[&str]) -> Result<()> {
    for name in names {
        let path = out_dir.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(CueError::new(
                    cue_core::PipelineStage::Render,
                    format!("could not remove stale output {}", path.display()),
                )
                .because(err.to_string()));
            }
        }
    }
    Ok(())
}

fn remember_successful_readiness(
    readiness: &mut Option<bool>,
    result: std::result::Result<bool, String>,
) -> std::result::Result<bool, String> {
    match result {
        Ok(ready) => {
            *readiness = Some(ready);
            Ok(ready)
        }
        Err(reason) => Err(reason),
    }
}

/// The two supported processing contracts.
///
/// Keeping this as an enum makes unsupported intermediate stopping points
/// unrepresentable while preserving the dedicated `cue transcribe` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessMode {
    Full,
    TranscriptOnly,
}

impl ProcessMode {
    fn includes(self, stage: cue_core::PipelineStage) -> bool {
        use cue_core::PipelineStage;

        match self {
            Self::Full => true,
            Self::TranscriptOnly => matches!(
                stage,
                PipelineStage::Inspect
                    | PipelineStage::Extract
                    | PipelineStage::Transcribe
                    | PipelineStage::Render
            ),
        }
    }
}

pub async fn run(cli: &Cue, config: &cue_core::Config) -> i32 {
    run_mode(cli, &cli.paths, ProcessMode::Full, config).await
}

/// Process files under one of the supported artifact contracts.
pub async fn run_mode(
    cli: &Cue,
    paths: &[PathBuf],
    mode: ProcessMode,
    config: &cue_core::Config,
) -> i32 {
    match run_inner(paths, mode, cli, config).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

async fn run_inner(
    paths: &[PathBuf],
    mode: ProcessMode,
    cli: &Cue,
    config: &cue_core::Config,
) -> Result<i32> {
    if paths.is_empty() {
        print_usage_hint();
        return Ok(0);
    }

    // Resolve the complete batch before starting any media work. This keeps
    // discovery and output collisions from producing partial batches.
    let plan = resolve_inputs(paths, cli.recursive, cli.output.as_deref().map(Path::new))?;
    let corrections = plan
        .inputs
        .iter()
        .map(|input| {
            CorrectionPlan::prepare(&input.output, cli.corrections.as_deref())
                .map(|correction| (input.output.clone(), correction))
        })
        .collect::<Result<HashMap<_, _>>>()?;

    // Stage logic emits events; the renderer decides presentation. Core
    // pipeline behavior never depends on terminal output.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let renderer = tokio::spawn(crate::events::run_renderer(rx));

    let mut processor = PipelineProcessor {
        cli,
        config,
        events: &tx,
        mode,
        corrections,
        s1_readiness: None,
    };
    let result =
        process_resolved_inputs(&plan.inputs, plan.is_batch, &mut processor, |input, err| {
            use cue_core::{PipelineEvent, PipelineStage};

            let _ = tx.send(PipelineEvent::Failed {
                stage: err.stage().unwrap_or(PipelineStage::Inspect),
                error: format!("{}: {err}", input.source.display()),
            });
        })
        .await;

    drop(tx); // close so the renderer can finish
    if let Err(join_err) = renderer.await {
        tracing::warn!(error = %join_err, "event renderer stopped early");
    }

    result.map(|outcome| {
        if plan.is_batch {
            println_line(&format!(
                "Batch complete: {} succeeded, {} failed",
                outcome.succeeded, outcome.failed
            ));
        }
        outcome.exit_code()
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
struct BatchOutcome {
    succeeded: usize,
    failed: usize,
}

impl BatchOutcome {
    fn exit_code(&self) -> i32 {
        i32::from(self.failed > 0)
    }
}

trait MediaProcessor {
    async fn process(&mut self, input: &ResolvedInput) -> Result<()>;
}

struct PipelineProcessor<'a> {
    cli: &'a Cue,
    config: &'a cue_core::Config,
    events: &'a tokio::sync::mpsc::UnboundedSender<cue_core::PipelineEvent>,
    mode: ProcessMode,
    corrections: HashMap<PathBuf, CorrectionPlan>,
    s1_readiness: Option<bool>,
}

impl MediaProcessor for PipelineProcessor<'_> {
    async fn process(&mut self, input: &ResolvedInput) -> Result<()> {
        let correction = self.corrections.get(&input.output).ok_or_else(|| {
            CueError::general(format!(
                "no correction plan prepared for {}",
                input.output.display()
            ))
        })?;
        let context = FileContext {
            cli: self.cli,
            config: self.config,
            events: self.events,
            mode: self.mode,
            correction,
        };
        process_file(
            &input.source,
            &input.output,
            &context,
            &mut self.s1_readiness,
        )
        .await
    }
}

async fn process_resolved_inputs<P, F>(
    inputs: &[ResolvedInput],
    is_batch: bool,
    processor: &mut P,
    mut on_failure: F,
) -> Result<BatchOutcome>
where
    P: MediaProcessor,
    F: FnMut(&ResolvedInput, &CueError),
{
    let mut outcome = BatchOutcome::default();
    for input in inputs {
        match processor.process(input).await {
            Ok(()) => outcome.succeeded += 1,
            Err(err) if is_batch => {
                outcome.failed += 1;
                on_failure(input, &err);
            }
            Err(err) => return Err(err),
        }
    }
    Ok(outcome)
}

struct FileContext<'a> {
    cli: &'a Cue,
    config: &'a cue_core::Config,
    events: &'a tokio::sync::mpsc::UnboundedSender<cue_core::PipelineEvent>,
    mode: ProcessMode,
    correction: &'a CorrectionPlan,
}

async fn process_file(
    path: &Path,
    out_dir: &Path,
    context: &FileContext<'_>,
    s1_readiness: &mut Option<bool>,
) -> Result<()> {
    use cue_core::{PipelineEvent, PipelineStage};

    let cli = context.cli;
    let config = context.config;
    let events = context.events;
    let mode = context.mode;
    let correction = context.correction;

    println_line(&format!("Processing {}...", path.display()));

    // ---- Inspect --------------------------------------------------------
    let ffprobe = require_tool("ffprobe", "inspect media files")?;
    let _ = events.send(PipelineEvent::Started(PipelineStage::Inspect));
    let media = cue_media::probe::inspect(&ffprobe, path).await?;
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
    let media_hash = cue_cache::file_hash(path)?;
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
        extract_audio(&ffmpeg, path, &wav_path).await?;
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
    let cached_transcript: Option<cue_core::Transcript> =
        load_cached::<cue_core::Transcript>(&transcript_cache, &transcript_cache_key).and_then(
            |cached| {
                if let Err(err) = cached.validate() {
                    tracing::warn!(error = %err, "ignoring invalid cached transcript");
                    None
                } else {
                    Some(cached)
                }
            },
        );
    let transcript = match cached_transcript {
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
            fresh.validate()?;
            store_cached(&transcript_cache, &transcript_cache_key, &fresh);
            let _ = events.send(PipelineEvent::Completed(PipelineStage::Transcribe));
            fresh
        }
    };

    // `cue transcribe` stops here: canonical transcript only.
    if !mode.includes(PipelineStage::Normalize) {
        let _ = events.send(PipelineEvent::Started(PipelineStage::Render));
        create_output_dir(out_dir)?;
        write_render_json(&out_dir.join("transcript.json"), &transcript)?;
        write_render_file(&out_dir.join("transcript.txt"), transcript.plain_text())?;
        correction.render(out_dir, config, CorrectionScope::TranscriptOnly, false)?;
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
        version: NORMALIZATION_CACHE_VERSION,
        transcript_hash: &transcript_hash,
        provider: &config.normalization.provider,
        endpoint: normalized_endpoint(&config.normalization.ollama_url),
        styling: &s1_settings.styling,
        structure: &s1_settings.structure,
        context: &s1_settings.context,
    };

    let normalized = match load_cached(&normalized_cache, &normalization_key) {
        Some(cached) => {
            let _ = events.send(PipelineEvent::Cached(PipelineStage::Normalize));
            Some(cached)
        }
        None => {
            let readiness = match *s1_readiness {
                Some(ready) => Ok(ready),
                None => {
                    let admin = cue_llm::OllamaAdmin::new(&config.normalization.ollama_url);
                    remember_successful_readiness(
                        s1_readiness,
                        cue_normalization::s1_ready(&admin)
                            .await
                            .map_err(|err| err.to_string()),
                    )
                }
            };
            let outcome = match readiness {
                Ok(true) => match cue_normalization::normalize_s1(
                    &config.normalization.ollama_url,
                    s1_settings.clone(),
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
                version: 3,
                normalized_hash: &clean_hash,
                language: &transcript.language,
                endpoint: normalized_endpoint(&llm.base_url),
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
    create_output_dir(out_dir)?;

    write_render_json(&out_dir.join("transcript.json"), &transcript)?;
    write_render_file(&out_dir.join("transcript.txt"), transcript.plain_text())?;

    if let Some(clean) = &normalized {
        write_render_json(&out_dir.join("normalized.json"), clean)?;
        write_render_file(&out_dir.join("transcript.clean.txt"), clean.plain_text())?;
    } else {
        remove_stale_artifacts(out_dir, &["normalized.json", "transcript.clean.txt"])?;
    }

    if let Some(analysis) = &analysis {
        write_render_json(&out_dir.join("analysis.json"), analysis)?;
        write_render_file(
            &out_dir.join("summary.md"),
            cue_analysis::render_summary(analysis),
        )?;
        write_render_file(
            &out_dir.join("description.md"),
            cue_analysis::render_description(analysis),
        )?;
    } else {
        remove_stale_artifacts(out_dir, &["analysis.json", "summary.md", "description.md"])?;
    }

    // Subtitles derive from the canonical transcript, never from cleaned
    // text.
    let policy = cue_subtitles::SubtitlePolicy {
        max_lines: config.subtitles.max_lines,
        max_chars_per_line: config.subtitles.max_chars_per_line,
        max_duration_ms: config.subtitles.max_duration_ms,
    };
    let cues = cue_subtitles::build_cues(&transcript, &policy)?;
    for format in &config.subtitles.formats {
        let path = out_dir.join(format!("subtitles.{}", format.extension()));
        let content = match format {
            cue_core::config::SubtitleFormat::Srt => cue_subtitles::render_srt(&cues),
            cue_core::config::SubtitleFormat::Vtt => cue_subtitles::render_vtt(&cues),
        };
        write_render_file(&path, content)?;
    }

    correction.render(out_dir, config, CorrectionScope::Full, false)?;

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

fn create_output_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|err| {
        CueError::new(
            cue_core::PipelineStage::Render,
            format!("could not create output directory {}", path.display()),
        )
        .because(err.to_string())
    })
}

fn write_render_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(value).map_err(|err| {
        CueError::new(cue_core::PipelineStage::Render, "serialization failed")
            .because(err.to_string())
    })?;
    write_render_file(path, json + "\n")
}

fn write_render_file(path: &Path, content: impl AsRef<[u8]>) -> Result<()> {
    std::fs::write(path, content).map_err(|err| {
        CueError::new(
            cue_core::PipelineStage::Render,
            format!("could not write {}", path.display()),
        )
        .because(err.to_string())
    })
}

fn print_usage_hint() {
    println_line("cue processes local video and audio files or directories.");
    println_line("\nUsage:");
    println_line("    cue <path>...");
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
    use std::collections::VecDeque;

    use super::*;

    struct StubProcessor {
        results: VecDeque<Result<()>>,
        attempted: Vec<PathBuf>,
    }

    impl MediaProcessor for StubProcessor {
        async fn process(&mut self, input: &ResolvedInput) -> Result<()> {
            self.attempted.push(input.source.clone());
            self.results.pop_front().expect("missing stub result")
        }
    }

    fn resolved(name: &str) -> ResolvedInput {
        ResolvedInput {
            source: PathBuf::from(name),
            output: PathBuf::from(format!("{name}.cue")),
        }
    }

    #[tokio::test]
    async fn batch_attempts_every_input_and_reports_failures() {
        let inputs = [resolved("one.mp4"), resolved("two.mp4")];
        let mut processor = StubProcessor {
            results: VecDeque::from([
                Err(CueError::general("first failed")),
                Err(CueError::general("second failed")),
            ]),
            attempted: Vec::new(),
        };
        let mut failures = Vec::new();

        let outcome = process_resolved_inputs(&inputs, true, &mut processor, |input, _| {
            failures.push(input.source.clone());
        })
        .await
        .unwrap();

        assert_eq!(
            processor.attempted,
            [PathBuf::from("one.mp4"), PathBuf::from("two.mp4")]
        );
        assert_eq!(failures, processor.attempted);
        assert_eq!(
            outcome,
            BatchOutcome {
                succeeded: 0,
                failed: 2
            }
        );
        assert_eq!(outcome.exit_code(), 1);
    }

    #[tokio::test]
    async fn successful_batch_returns_zero() {
        let inputs = [resolved("one.mp4"), resolved("two.mp4")];
        let mut processor = StubProcessor {
            results: VecDeque::from([Ok(()), Ok(())]),
            attempted: Vec::new(),
        };

        let outcome = process_resolved_inputs(&inputs, true, &mut processor, |_, _| {})
            .await
            .unwrap();

        assert_eq!(
            outcome,
            BatchOutcome {
                succeeded: 2,
                failed: 0
            }
        );
        assert_eq!(outcome.exit_code(), 0);
    }

    #[tokio::test]
    async fn single_input_propagates_failure_without_callback_or_continuation() {
        let inputs = [resolved("one.mp4"), resolved("two.mp4")];
        let mut processor = StubProcessor {
            results: VecDeque::from([Err(CueError::general("first failed")), Ok(())]),
            attempted: Vec::new(),
        };
        let mut failures = Vec::new();

        let err = process_resolved_inputs(&inputs, false, &mut processor, |input, _| {
            failures.push(input.source.clone());
        })
        .await
        .unwrap_err();

        assert!(err.to_string().contains("first failed"));
        assert_eq!(processor.attempted, [PathBuf::from("one.mp4")]);
        assert!(failures.is_empty());
    }

    #[tokio::test]
    async fn mixed_batch_attempts_all_inputs_and_reports_only_the_failure() {
        let inputs = [
            resolved("one.mp4"),
            resolved("two.mp4"),
            resolved("three.mp4"),
        ];
        let mut processor = StubProcessor {
            results: VecDeque::from([Ok(()), Err(CueError::general("second failed")), Ok(())]),
            attempted: Vec::new(),
        };
        let mut failures = Vec::new();

        let outcome = process_resolved_inputs(&inputs, true, &mut processor, |input, _| {
            failures.push(input.source.clone());
        })
        .await
        .unwrap();

        assert_eq!(
            processor.attempted,
            [
                PathBuf::from("one.mp4"),
                PathBuf::from("two.mp4"),
                PathBuf::from("three.mp4")
            ]
        );
        assert_eq!(failures, [PathBuf::from("two.mp4")]);
        assert_eq!(
            outcome,
            BatchOutcome {
                succeeded: 2,
                failed: 1
            }
        );
        assert_eq!(outcome.exit_code(), 1);
    }

    #[test]
    fn output_directory_failures_are_attributed_to_render() {
        let dir = tempfile::tempdir().unwrap();
        let parent_file = dir.path().join("not-a-directory");
        std::fs::write(&parent_file, "occupied").unwrap();

        let err = create_output_dir(&parent_file.join("output")).unwrap_err();

        assert_eq!(err.stage(), Some(cue_core::PipelineStage::Render));
    }

    #[test]
    fn artifact_write_failures_are_attributed_to_render() {
        let dir = tempfile::tempdir().unwrap();

        let text_err = write_render_file(dir.path(), "transcript").unwrap_err();
        let json_err =
            write_render_json(dir.path(), &serde_json::json!({"text": "hello"})).unwrap_err();

        assert_eq!(text_err.stage(), Some(cue_core::PipelineStage::Render));
        assert_eq!(json_err.stage(), Some(cue_core::PipelineStage::Render));
    }

    #[test]
    fn analysis_cache_identity_includes_canonical_language() {
        let english = AnalysisCacheKey {
            version: 3,
            normalized_hash: "same-normalized-text",
            language: "en",
            endpoint: "https://gateway.example/v1",
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

    #[test]
    fn provider_endpoints_participate_in_cache_identity() {
        let normalization_a = NormalizationCacheKey {
            version: NORMALIZATION_CACHE_VERSION,
            transcript_hash: "transcript",
            provider: "ollama",
            endpoint: normalized_endpoint("http://ollama-a:11434/"),
            styling: "style",
            structure: "structure",
            context: "context",
        };
        let normalization_b = NormalizationCacheKey {
            endpoint: normalized_endpoint("http://ollama-b:11434"),
            ..normalization_a
        };
        assert_ne!(
            serde_json::to_vec(&normalization_a).unwrap(),
            serde_json::to_vec(&normalization_b).unwrap()
        );

        let analysis_a = AnalysisCacheKey {
            version: 3,
            normalized_hash: "normalized",
            language: "en",
            endpoint: normalized_endpoint("https://gateway-a.example/v1/"),
            model: "model",
            prompt_version: cue_analysis::PROMPT_VERSION,
        };
        let analysis_b = AnalysisCacheKey {
            endpoint: normalized_endpoint("https://gateway-b.example/v1"),
            ..analysis_a
        };
        assert_ne!(
            serde_json::to_vec(&analysis_a).unwrap(),
            serde_json::to_vec(&analysis_b).unwrap()
        );

        let analysis_a_with_slash = AnalysisCacheKey {
            endpoint: normalized_endpoint("https://gateway-a.example/v1/"),
            ..analysis_a
        };
        assert_eq!(
            serde_json::to_vec(&analysis_a).unwrap(),
            serde_json::to_vec(&analysis_a_with_slash).unwrap()
        );
    }

    #[test]
    fn normalization_cache_version_invalidates_v2_entries() {
        let current = NormalizationCacheKey {
            version: NORMALIZATION_CACHE_VERSION,
            transcript_hash: "transcript",
            provider: "ollama",
            endpoint: "http://localhost:11434",
            styling: "style",
            structure: "structure",
            context: "context",
        };
        let legacy = NormalizationCacheKey {
            version: 2,
            ..current
        };

        assert_eq!(NORMALIZATION_CACHE_VERSION, 3);
        assert_ne!(
            serde_json::to_vec(&current).unwrap(),
            serde_json::to_vec(&legacy).unwrap()
        );
    }

    #[test]
    fn removes_stale_optional_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["normalized.json", "transcript.clean.txt", "analysis.json"] {
            std::fs::write(dir.path().join(name), "stale").unwrap();
        }

        remove_stale_artifacts(
            dir.path(),
            &["normalized.json", "transcript.clean.txt", "analysis.json"],
        )
        .unwrap();

        assert!(!dir.path().join("normalized.json").exists());
        assert!(!dir.path().join("transcript.clean.txt").exists());
        assert!(!dir.path().join("analysis.json").exists());
        remove_stale_artifacts(dir.path(), &["normalized.json"]).unwrap();
    }

    #[test]
    fn readiness_errors_are_not_remembered_for_later_files() {
        let mut readiness = None;

        let first = remember_successful_readiness(&mut readiness, Err("temporary outage".into()));
        assert_eq!(first.unwrap_err(), "temporary outage");
        assert_eq!(readiness, None);

        let second = remember_successful_readiness(&mut readiness, Ok(true));
        assert!(second.unwrap());
        assert_eq!(readiness, Some(true));
    }

    #[test]
    fn process_modes_define_complete_or_transcript_only_stage_contracts() {
        use cue_core::PipelineStage;

        assert!(
            PipelineStage::ALL
                .into_iter()
                .all(|stage| ProcessMode::Full.includes(stage))
        );
        assert!(ProcessMode::TranscriptOnly.includes(PipelineStage::Inspect));
        assert!(ProcessMode::TranscriptOnly.includes(PipelineStage::Extract));
        assert!(ProcessMode::TranscriptOnly.includes(PipelineStage::Transcribe));
        assert!(ProcessMode::TranscriptOnly.includes(PipelineStage::Render));
        assert!(!ProcessMode::TranscriptOnly.includes(PipelineStage::Normalize));
        assert!(!ProcessMode::TranscriptOnly.includes(PipelineStage::Analyze));
    }
}
