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
use cue_core::config::LlmCredentialReadiness;
use cue_core::media::Media;
use cue_core::{CueError, PipelineStage, Result};
use cue_media::extract::extract_audio;
use cue_transcription::Transcriber;

use crate::cli::Cue;
use crate::commands::batch::{KeyedLocks, MediaProcessor, process_inputs};
use crate::commands::inputs::{ResolvedInput, resolve_inputs};
use crate::corrections::{CorrectionPlan, CorrectionScope};
use crate::events::{FileEvents, FilePipelineEvent, RendererEvent};
use crate::render::{human_duration, println_line};
use crate::run_contract::{
    ProcessModeName, ProviderIdentity, RemoteDataUsage, RunReceipt, StageRecord, StageStatus,
    TrackedFile,
};

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

fn public_normalization_skip_reason(_internal_reason: &str) -> &'static str {
    "normalization unavailable; check the configured Ollama service and S1 model setup"
}

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

    fn receipt_name(self) -> ProcessModeName {
        match self {
            Self::Full => ProcessModeName::Full,
            Self::TranscriptOnly => ProcessModeName::TranscriptOnly,
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
    preflight_summary(mode, cli.summary, config)?;
    if mode == ProcessMode::Full {
        for input in &plan.inputs {
            let layout = crate::commands::output::OutputLayout {
                workspace: input.workspace.clone(),
                published_base: input.published_base.clone(),
            };
            crate::commands::output::preflight_subtitles(
                &layout,
                config.subtitles.formats.iter().copied(),
                false,
            )?;
        }
    }
    let corrections = CorrectionPlan::prepare_batch(
        plan.inputs.iter().map(|input| input.workspace.as_path()),
        cli.corrections.as_deref(),
    )?;

    // Stage logic emits events; the renderer decides presentation. Core
    // pipeline behavior never depends on terminal output.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let renderer = tokio::spawn(crate::events::run_renderer(rx, plan.is_batch, cli.summary));

    let processor = PipelineProcessor {
        cli,
        config,
        events: &tx,
        mode,
        corrections,
        s1_readiness: tokio::sync::OnceCell::new(),
        cache_work: KeyedLocks::default(),
    };
    let result = process_inputs(
        &plan.inputs,
        plan.is_batch,
        cli.jobs,
        &processor,
        |input, result| {
            if cli.summary && cli.stream {
                print_summary(input, result);
            }
            cli.summary && !cli.stream
        },
    )
    .await;

    if let Ok(outcome) = &result {
        for failure in &outcome.failures {
            use cue_core::{PipelineEvent, PipelineStage};

            let _ = tx.send(FilePipelineEvent {
                source: failure.input.source.clone(),
                event: RendererEvent::Pipeline(PipelineEvent::Failed {
                    stage: failure.error.stage().unwrap_or(PipelineStage::Inspect),
                    error: failure.error.to_string(),
                }),
            });
        }
    }

    drop(tx); // close so the renderer can finish
    if let Err(join_err) = renderer.await {
        tracing::warn!(error = %join_err, "event renderer stopped early");
    }

    result.map(|outcome| {
        if cli.summary && !cli.stream {
            for success in &outcome.successes {
                print_summary(success.input, &success.value);
            }
        }
        if plan.is_batch {
            let batch_line = format!(
                "Batch complete: {} succeeded, {} failed",
                outcome.succeeded(),
                outcome.failures.len()
            );
            if cli.summary {
                eprintln!("{batch_line}");
            } else {
                println_line(&batch_line);
            }
        }
        outcome.exit_code()
    })
}

fn preflight_summary(mode: ProcessMode, summary: bool, config: &cue_core::Config) -> Result<()> {
    if !summary || !mode.includes(PipelineStage::Analyze) {
        return Ok(());
    }

    let Some(llm) = &config.llm else {
        return Err(CueError::new(
            PipelineStage::Analyze,
            "cannot produce the requested summary because no LLM gateway is configured",
        )
        .remedy("configure the [llm] gateway in cue.toml and retry"));
    };

    if let LlmCredentialReadiness::Missing { api_key_env } = llm.credential_readiness() {
        return Err(CueError::new(
            PipelineStage::Analyze,
            format!("cannot produce the requested summary because {api_key_env} is not set"),
        )
        .remedy(format!(
            "set {api_key_env}, or configure api_key_env = \"\" for an unauthenticated gateway"
        )));
    }

    Ok(())
}

fn print_summary(input: &ResolvedInput, result: &ProcessResult) {
    if let Some(summary) = &result.summary {
        println_line(&format!(
            "==> {} <==\n\n{}",
            input.source.display(),
            summary.trim()
        ));
    }
}

struct ProcessResult {
    summary: Option<String>,
}

struct PipelineProcessor<'a> {
    cli: &'a Cue,
    config: &'a cue_core::Config,
    events: &'a tokio::sync::mpsc::UnboundedSender<FilePipelineEvent>,
    mode: ProcessMode,
    corrections: HashMap<PathBuf, CorrectionPlan>,
    s1_readiness: tokio::sync::OnceCell<bool>,
    cache_work: KeyedLocks,
}

impl MediaProcessor for PipelineProcessor<'_> {
    type Output = ProcessResult;

    async fn process(&self, input: &ResolvedInput) -> Result<ProcessResult> {
        let correction = self.corrections.get(&input.workspace).ok_or_else(|| {
            CueError::general(format!(
                "no correction plan prepared for {}",
                input.workspace.display()
            ))
        })?;
        let context = FileContext {
            cli: self.cli,
            config: self.config,
            events: FileEvents::new(input.source.clone(), self.events.clone()),
            mode: self.mode,
            correction,
            cache_work: &self.cache_work,
        };
        process_file(input, &context, &self.s1_readiness).await
    }
}

struct FileContext<'a> {
    cli: &'a Cue,
    config: &'a cue_core::Config,
    events: FileEvents,
    mode: ProcessMode,
    correction: &'a CorrectionPlan,
    cache_work: &'a KeyedLocks,
}

fn initial_run_receipt(
    source: &Path,
    output_dir: &Path,
    source_hash: String,
    context: &FileContext<'_>,
) -> Result<RunReceipt> {
    let FileContext {
        cli,
        config,
        mode,
        correction,
        ..
    } = context;
    let mut providers = vec![
        ProviderIdentity {
            stage: cue_core::PipelineStage::Inspect,
            provider: "ffprobe".into(),
            model: None,
            endpoint: None,
        },
        ProviderIdentity {
            stage: cue_core::PipelineStage::Extract,
            provider: "ffmpeg".into(),
            model: None,
            endpoint: None,
        },
        ProviderIdentity {
            stage: cue_core::PipelineStage::Transcribe,
            provider: "faster-whisper".into(),
            model: Some(config.transcription.model.clone()),
            endpoint: None,
        },
    ];
    if *mode == ProcessMode::Full {
        providers.push(ProviderIdentity {
            stage: cue_core::PipelineStage::Normalize,
            provider: config.normalization.provider.clone(),
            model: Some(cue_normalization::S1_MODEL_NAME.into()),
            endpoint: Some(crate::run_contract::sanitize_endpoint(
                &config.normalization.ollama_url,
            )),
        });
        if let Some(llm) = &config.llm {
            providers.push(ProviderIdentity {
                stage: cue_core::PipelineStage::Analyze,
                provider: "openai-compatible".into(),
                model: Some(llm.model.clone()),
                endpoint: Some(crate::run_contract::sanitize_endpoint(&llm.base_url)),
            });
        }
    }
    Ok(RunReceipt {
        schema_version: crate::run_contract::SCHEMA_VERSION,
        cue_version: env!("CARGO_PKG_VERSION").into(),
        mode: mode.receipt_name(),
        source: TrackedFile::from_digest(
            crate::run_contract::tracked_reference(output_dir, source)?,
            source_hash,
        ),
        configuration: crate::run_contract::configuration_snapshot(config, cli.language.as_deref()),
        providers,
        stages: vec![StageRecord::new(
            cue_core::PipelineStage::Inspect,
            StageStatus::Executed,
            None,
        )],
        warnings: Vec::new(),
        remote_data_usage: RemoteDataUsage {
            normalized_text_sent_to_remote_in_current_run: None,
        },
        corrections: correction.attested_manifests(output_dir)?,
        artifacts: Vec::new(),
        published_outputs: Vec::new(),
    })
}

fn include_if_present(output_dir: &Path, names: &mut Vec<String>, name: &str) {
    if output_dir.join(name).is_file() {
        names.push(name.into());
    }
}

async fn process_file(
    input: &ResolvedInput,
    context: &FileContext<'_>,
    s1_readiness: &tokio::sync::OnceCell<bool>,
) -> Result<ProcessResult> {
    use cue_core::{PipelineEvent, PipelineStage};

    let cli = context.cli;
    let config = context.config;
    let events = &context.events;
    let mode = context.mode;
    let correction = context.correction;
    let path = &input.source;
    let out_dir = &input.workspace;

    events.processing();

    // ---- Inspect --------------------------------------------------------
    let ffprobe = require_tool("ffprobe", "inspect media files")?;
    events.send(PipelineEvent::Started(PipelineStage::Inspect));
    let media = cue_media::probe::inspect(&ffprobe, path).await?;
    for line in media_summary(&media) {
        events.message(line);
    }
    events.send(PipelineEvent::Completed(PipelineStage::Inspect));

    if !media.has_audio() {
        return Err(CueError::new(
            cue_core::PipelineStage::Extract,
            format!("{} contains no audio stream", path.display()),
        )
        .remedy("cue can only process files that contain audio"));
    }

    // Content cache layout keyed by the media bytes themselves.
    let media_hash = cue_cache::file_hash(path)?;
    let cache_work = context.cache_work.lock(&media_hash).await;
    let mut run_receipt = initial_run_receipt(path, out_dir, media_hash.clone(), context)?;
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
        events.send(PipelineEvent::Cached(PipelineStage::Extract));
        run_receipt.stages.push(StageRecord::new(
            PipelineStage::Extract,
            StageStatus::Cached,
            None,
        ));
    } else {
        events.send(PipelineEvent::Started(PipelineStage::Extract));
        extract_audio(&ffmpeg, path, &wav_path).await?;
        events.send(PipelineEvent::Completed(PipelineStage::Extract));
        run_receipt.stages.push(StageRecord::new(
            PipelineStage::Extract,
            StageStatus::Executed,
            None,
        ));
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
            events.send(PipelineEvent::Cached(PipelineStage::Transcribe));
            run_receipt.stages.push(StageRecord::new(
                PipelineStage::Transcribe,
                StageStatus::Cached,
                None,
            ));
            cached
        }
        None => {
            events.send(PipelineEvent::Started(PipelineStage::Transcribe));
            let transcriber = cue_transcription::FasterWhisperTranscriber::resolve(None)?;
            let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
            let transcription =
                transcriber.transcribe_with_progress(&wav_path, &options, Some(progress_tx));
            tokio::pin!(transcription);
            let mut progress_open = true;
            let fresh = loop {
                tokio::select! {
                    result = &mut transcription => break result?,
                    progress = progress_rx.recv(), if progress_open => match progress {
                        Some(percent) => events.send(PipelineEvent::Progress {
                            stage: PipelineStage::Transcribe,
                            percent,
                        }),
                        None => progress_open = false,
                    }
                }
            };
            fresh.validate()?;
            store_cached(&transcript_cache, &transcript_cache_key, &fresh);
            events.send(PipelineEvent::Completed(PipelineStage::Transcribe));
            run_receipt.stages.push(StageRecord::new(
                PipelineStage::Transcribe,
                StageStatus::Executed,
                None,
            ));
            fresh
        }
    };

    // `cue transcribe` stops here: canonical transcript only.
    if !mode.includes(PipelineStage::Normalize) {
        drop(cache_work);
        events.send(PipelineEvent::Started(PipelineStage::Render));
        let _output_lock = crate::run_contract::OutputLock::acquire(out_dir)?;
        begin_render(out_dir, correction)?;
        crate::commands::output::write_workspace_descriptor(out_dir, path)?;
        write_render_json(&out_dir.join("transcript.json"), &transcript)?;
        write_render_file(&out_dir.join("transcript.txt"), transcript.plain_text())?;
        correction.render(out_dir, config, CorrectionScope::TranscriptOnly, false)?;
        run_receipt.stages.push(StageRecord::new(
            PipelineStage::Render,
            StageStatus::Executed,
            None,
        ));
        let mut artifacts = vec![
            crate::commands::output::WORKSPACE_FILE.into(),
            "transcript.json".into(),
            "transcript.txt".into(),
        ];
        include_if_present(out_dir, &mut artifacts, "corrections.applied.json");
        run_receipt.publish(out_dir, &artifacts)?;
        events.send(PipelineEvent::Completed(PipelineStage::Render));
        events.message(format!(
            "\nDone. Transcript written to {}/",
            out_dir.display()
        ));
        return Ok(ProcessResult { summary: None });
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
            events.send(PipelineEvent::Cached(PipelineStage::Normalize));
            run_receipt.stages.push(StageRecord::new(
                PipelineStage::Normalize,
                StageStatus::Cached,
                None,
            ));
            Some(cached)
        }
        None => {
            let readiness = s1_readiness
                .get_or_try_init(|| async {
                    let admin = cue_llm::OllamaAdmin::new(&config.normalization.ollama_url);
                    cue_normalization::s1_ready(&admin)
                        .await
                        .map_err(|err| err.to_string())
                })
                .await
                .copied();
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
                    events.send(PipelineEvent::Completed(PipelineStage::Normalize));
                    run_receipt.stages.push(StageRecord::new(
                        PipelineStage::Normalize,
                        StageStatus::Executed,
                        None,
                    ));
                    Some(clean)
                }
                cue_normalization::NormalizationOutcome::Skipped(reason) => {
                    let public_reason = public_normalization_skip_reason(&reason);
                    tracing::info!(reason = public_reason, "normalization skipped");
                    run_receipt.stages.push(StageRecord::new(
                        PipelineStage::Normalize,
                        StageStatus::Skipped,
                        Some(public_reason.into()),
                    ));
                    run_receipt
                        .warnings
                        .push(format!("normalization skipped: {public_reason}"));
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
                    events.send(PipelineEvent::Cached(PipelineStage::Analyze));
                    run_receipt.stages.push(StageRecord::new(
                        PipelineStage::Analyze,
                        StageStatus::Cached,
                        None,
                    ));
                    Some(cached)
                }
                None => {
                    events.send(PipelineEvent::Started(PipelineStage::Analyze));
                    if crate::run_contract::endpoint_is_remote(&llm.base_url) {
                        run_receipt
                            .remote_data_usage
                            .normalized_text_sent_to_remote_in_current_run =
                            Some(crate::run_contract::sanitize_endpoint(&llm.base_url));
                    }
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
                            events.send(PipelineEvent::Completed(PipelineStage::Analyze));
                            run_receipt.stages.push(StageRecord::new(
                                PipelineStage::Analyze,
                                StageStatus::Executed,
                                None,
                            ));
                            Some(a)
                        }
                        Err(err) => {
                            events.send(PipelineEvent::Failed {
                                stage: PipelineStage::Analyze,
                                error: err.to_string(),
                            });
                            tracing::warn!(error = %err, "analysis failed");
                            run_receipt.stages.push(StageRecord::new(
                                PipelineStage::Analyze,
                                StageStatus::Degraded,
                                Some("analysis request failed; see run logs".into()),
                            ));
                            run_receipt
                                .warnings
                                .push("analysis failed; see run logs".into());
                            None
                        }
                    }
                }
            }
        }
        (None, _) => {
            tracing::info!("analysis skipped: no LLM gateway configured");
            run_receipt.stages.push(StageRecord::new(
                PipelineStage::Analyze,
                StageStatus::Skipped,
                Some("no LLM gateway configured".into()),
            ));
            None
        }
        (Some(_), None) => {
            tracing::info!("analysis skipped: no cleaned text (S1 unavailable)");
            run_receipt.stages.push(StageRecord::new(
                PipelineStage::Analyze,
                StageStatus::Skipped,
                Some("no cleaned text (S1 unavailable)".into()),
            ));
            None
        }
    };

    // ---- Render ---------------------------------------------------------
    drop(cache_work);
    events.send(PipelineEvent::Started(PipelineStage::Render));
    let _output_lock = crate::run_contract::OutputLock::acquire(out_dir)?;
    let layout = crate::commands::output::OutputLayout {
        workspace: input.workspace.clone(),
        published_base: input.published_base.clone(),
    };
    let previous_published = crate::commands::output::owned_published_outputs(&layout);
    begin_render(out_dir, correction)?;
    crate::commands::output::write_workspace_descriptor(out_dir, path)?;

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
    for format in [
        cue_core::config::SubtitleFormat::Srt,
        cue_core::config::SubtitleFormat::Vtt,
    ] {
        if !config.subtitles.formats.contains(&format) {
            let name = format!("subtitles.{}", format.extension());
            remove_stale_artifacts(out_dir, &[&name])?;
        }
    }
    for format in &config.subtitles.formats {
        let path = out_dir.join(format!("subtitles.{}", format.extension()));
        let content = match format {
            cue_core::config::SubtitleFormat::Srt => cue_subtitles::render_srt(&cues),
            cue_core::config::SubtitleFormat::Vtt => cue_subtitles::render_vtt(&cues),
        };
        write_render_file(&path, content)?;
    }

    correction.render(out_dir, config, CorrectionScope::Full, false)?;

    run_receipt.ensure_inputs_current(out_dir)?;
    let published_outputs = crate::commands::output::publish_subtitles(
        &layout,
        config.subtitles.formats.iter().copied(),
        false,
    )?;

    run_receipt.stages.push(StageRecord::new(
        PipelineStage::Render,
        StageStatus::Executed,
        None,
    ));
    let mut artifacts = vec![
        crate::commands::output::WORKSPACE_FILE.into(),
        "transcript.json".into(),
        "transcript.txt".into(),
    ];
    if normalized.is_some() {
        artifacts.extend(["normalized.json".into(), "transcript.clean.txt".into()]);
    }
    if analysis.is_some() {
        artifacts.extend([
            "analysis.json".into(),
            "summary.md".into(),
            "description.md".into(),
        ]);
    }
    for format in &config.subtitles.formats {
        artifacts.push(format!("subtitles.{}", format.extension()));
    }
    include_if_present(out_dir, &mut artifacts, "corrections.applied.json");
    run_receipt.publish_with_outputs(out_dir, &artifacts, &published_outputs)?;
    crate::commands::output::remove_stale_published_outputs(
        &previous_published,
        &published_outputs,
    )?;

    events.send(PipelineEvent::Completed(PipelineStage::Render));
    events.message(format!(
        "\nDone. Transcript and subtitles written to {}/",
        out_dir.display()
    ));
    if cli.summary {
        let Some(analysis) = &analysis else {
            return Err(CueError::new(
                PipelineStage::Analyze,
                format!("could not produce requested summary for {}", path.display()),
            )
            .remedy("configure a working analysis gateway and local S1 normalization model"));
        };
        return Ok(ProcessResult {
            summary: Some(cue_analysis::render_summary(analysis)),
        });
    }

    Ok(ProcessResult { summary: None })
}

fn require_tool(binary: &str, purpose: &str) -> Result<PathBuf> {
    cue_media::tools::find_on_path(binary).ok_or_else(|| {
        CueError::general(format!("{} is required to {purpose}", capitalize(binary)))
            .because(format!("{binary} was not found on PATH"))
            .remedy("install FFmpeg and verify with `cue doctor`")
    })
}

fn begin_render(output_dir: &Path, correction: &CorrectionPlan) -> Result<()> {
    create_output_dir(output_dir)?;
    crate::run_contract::invalidate(output_dir)?;
    correction.invalidate_receipt(output_dir)
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

fn media_summary(media: &Media) -> Vec<String> {
    let mut lines = vec![
        format!("  format:     {}", media.format),
        format!("  duration:   {}", human_duration(media.duration_ms)),
    ];

    if media.has_video() {
        for stream in &media.video_streams {
            lines.push(format!(
                "  video:      #{} {} ({}x{} @ {})",
                stream.index, stream.codec, stream.width, stream.height, stream.frame_rate
            ));
        }
    } else {
        lines.push("  video:      none".into());
    }

    for stream in &media.audio_streams {
        lines.push(format!(
            "  audio:      #{} {} ({} Hz, {} ch)",
            stream.index, stream.codec, stream.sample_rate_hz, stream.channels
        ));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn render_setup_invalidates_a_stale_receipt_before_artifact_writes() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("lesson.cue");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("corrections.applied.json"), "STALE\n").unwrap();
        std::fs::write(output.join(crate::run_contract::RECEIPT_FILE), "STALE\n").unwrap();
        std::fs::create_dir(output.join("transcript.json")).unwrap();
        let correction = CorrectionPlan::prepare(&output, None).unwrap();

        begin_render(&output, &correction).unwrap();
        let error = write_render_json(
            &output.join("transcript.json"),
            &serde_json::json!({"text": "new render"}),
        )
        .unwrap_err();

        assert_eq!(error.stage(), Some(cue_core::PipelineStage::Render));
        assert!(!output.join("corrections.applied.json").exists());
        assert!(!output.join(crate::run_contract::RECEIPT_FILE).exists());
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
    fn normalization_skip_reason_does_not_expose_transport_details() {
        let internal_reason =
            "request failed for https://admin:secret@example.test/api/normalize?token=credential";

        let public_reason = public_normalization_skip_reason(internal_reason);

        assert_eq!(
            public_reason,
            "normalization unavailable; check the configured Ollama service and S1 model setup"
        );
        assert!(!public_reason.contains("admin:secret"));
        assert!(!public_reason.contains("example.test"));
        assert!(!public_reason.contains("credential"));
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
