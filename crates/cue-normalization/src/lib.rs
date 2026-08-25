//! Transcript normalization behind a provider-independent trait.

pub mod chunk;
pub mod ollama_cli;
pub mod s1;

use std::path::PathBuf;

use async_trait::async_trait;
use cue_core::{CueError, NormalizedTranscript, PipelineStage, Result, Transcript};

pub use chunk::{TranscriptChunk, chunk_transcript};
pub use ollama_cli::OllamaCli;
pub use s1::{S1Normalizer, S1Settings};

/// A provider that rewrites raw transcript text into clean prose.
///
/// Implementations must preserve the input's time coverage: every output
/// chunk maps onto the time range of the input text it was derived from.
#[async_trait]
pub trait TranscriptNormalizer: Send + Sync {
    /// Human-readable provider name for logs and cache keys.
    fn name(&self) -> &str;

    async fn normalize(&self, transcript: &Transcript) -> Result<NormalizedTranscript>;
}

/// Where the pinned S1 Modelfile lives (embedded for `models install`).
pub const S1_MODELFILE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../models/s1/Modelfile"
));

/// Name the created model carries in Ollama.
pub const S1_MODEL_NAME: &str = "cue-s1-mini";

/// Reference of the GGUF the Modelfile builds from.
pub const S1_SOURCE_REF: &str = "hf.co/superwhisper/s1-mini-GGUF:Q4_K_M";

/// Cue's data directory: `$CUE_DATA_DIR`, else `$XDG_DATA_HOME/cue`, else
/// `~/.local/share/cue`.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CUE_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .map(PathBuf::from)
                .map(|h| h.join(".local/share"))
        })?;
    Some(base.join("cue"))
}

/// Write the embedded S1 Modelfile into the data dir so `ollama create`
/// can read it (the CLI does not accept the Modelfile on stdin).
pub fn materialize_modelfile() -> Result<PathBuf> {
    let dir = data_dir().ok_or_else(|| {
        CueError::new(
            PipelineStage::Normalize,
            "could not determine cue's data directory",
        )
        .remedy("set CUE_DATA_DIR to a writable directory")
    })?;
    let path = dir.join("models").join("s1").join("Modelfile");
    std::fs::create_dir_all(path.parent().expect("parent exists")).map_err(|e| {
        CueError::new(PipelineStage::Normalize, "could not create model directory")
            .because(e.to_string())
    })?;
    let stale = std::fs::read_to_string(&path)
        .map(|existing| existing != S1_MODELFILE)
        .unwrap_or(true);
    if stale {
        std::fs::write(&path, S1_MODELFILE).map_err(|e| {
            CueError::new(
                PipelineStage::Normalize,
                format!("could not write S1 Modelfile to {}", path.display()),
            )
            .because(e.to_string())
        })?;
    }
    Ok(path)
}

/// Convenience: install or verify the S1 model in Ollama.
///
/// Pulls the source GGUF when missing, then creates the named model from
/// cue's embedded Modelfile via the `ollama` CLI (the HTTP create API
/// rejects `FROM hf.co/...` Modelfiles). Safe to run repeatedly.
pub async fn install_s1(admin: &cue_llm::OllamaAdmin) -> Result<String> {
    if admin.has_model(S1_MODEL_NAME).await? {
        return Ok(format!("{S1_MODEL_NAME} is already installed"));
    }
    let cli = OllamaCli::new(admin.base_url());
    if !admin.has_model(S1_SOURCE_REF).await? {
        cli.pull(S1_SOURCE_REF).await?;
    }
    let modelfile = materialize_modelfile()?;
    cli.create(S1_MODEL_NAME, &modelfile).await?;
    Ok(format!("installed {S1_MODEL_NAME}"))
}

/// Check whether Ollama has the S1 model ready.
pub async fn s1_ready(admin: &cue_llm::OllamaAdmin) -> bool {
    admin.has_model(S1_MODEL_NAME).await.unwrap_or(false)
}

/// Result of attempting normalization in the pipeline: either cleaned text
/// or a human-readable reason it was skipped. Normalization never fails the
/// pipeline.
pub enum NormalizationOutcome {
    Done(NormalizedTranscript),
    Skipped(String),
}

/// Normalize via S1 when available; produce a skip reason otherwise.
pub async fn normalize_if_ready(
    ollama_url: &str,
    settings: S1Settings,
    transcript: &Transcript,
) -> NormalizationOutcome {
    let admin = cue_llm::OllamaAdmin::new(ollama_url);
    if !s1_ready(&admin).await {
        return NormalizationOutcome::Skipped(format!(
            "model \"{S1_MODEL_NAME}\" not found in Ollama"
        ));
    }

    match S1Normalizer::new(ollama_url)
        .with_settings(settings)
        .normalize(transcript)
        .await
    {
        Ok(clean) => NormalizationOutcome::Done(clean),
        Err(err) => NormalizationOutcome::Skipped(err.to_string()),
    }
}
