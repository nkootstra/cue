//! Transcript normalization behind a provider-independent trait.

pub mod chunk;
pub mod ollama_cli;
pub mod s1;

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use cue_core::{CueError, NormalizedTranscript, PipelineStage, Result, Transcript};

pub use chunk::{TranscriptChunk, chunk_transcript};
pub use cue_core::paths::data_dir;
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

/// Write the embedded S1 Modelfile into the data dir so `ollama create`
/// can read it (the CLI does not accept the Modelfile on stdin).
pub fn materialize_modelfile() -> Result<PathBuf> {
    let data_dir = data_dir().ok_or_else(|| {
        CueError::new(
            PipelineStage::Normalize,
            "could not determine cue's data directory",
        )
        .remedy("set CUE_DATA_DIR to a writable directory")
    })?;
    materialize_modelfile_in(&data_dir)
}

fn materialize_modelfile_in(data_dir: &Path) -> Result<PathBuf> {
    let path = data_dir.join("models").join("s1").join("Modelfile");
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
    let models = admin.list_models().await?;
    if cue_llm::OllamaAdmin::models_include(&models, S1_MODEL_NAME) {
        return Ok(format!("{S1_MODEL_NAME} is already installed"));
    }
    let cli = OllamaCli::new(admin.base_url());
    if !cue_llm::OllamaAdmin::models_include(&models, S1_SOURCE_REF) {
        cli.pull(S1_SOURCE_REF).await?;
    }
    let modelfile = materialize_modelfile()?;
    cli.create(S1_MODEL_NAME, &modelfile).await?;
    Ok(format!("installed {S1_MODEL_NAME}"))
}

/// Check whether Ollama has the S1 model ready without hiding probe errors.
pub async fn s1_ready(admin: &cue_llm::OllamaAdmin) -> Result<bool> {
    Ok(s1_ready_in(&admin.list_models().await?))
}

/// Derive S1 readiness from an existing model snapshot.
pub fn s1_ready_in(models: &[cue_llm::OllamaModel]) -> bool {
    cue_llm::OllamaAdmin::models_include(models, S1_MODEL_NAME)
}

/// Normalize with S1 after the caller has established readiness.
pub async fn normalize_s1(
    ollama_url: &str,
    settings: S1Settings,
    transcript: &Transcript,
) -> Result<NormalizedTranscript> {
    S1Normalizer::new(ollama_url)
        .with_settings(settings)
        .normalize(transcript)
        .await
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
    match s1_ready(&admin).await {
        Ok(true) => {}
        Ok(false) => {
            return NormalizationOutcome::Skipped(format!(
                "model \"{S1_MODEL_NAME}\" not found in Ollama"
            ));
        }
        Err(err) => return NormalizationOutcome::Skipped(err.to_string()),
    }

    match normalize_s1(ollama_url, settings, transcript).await {
        Ok(clean) => NormalizationOutcome::Done(clean),
        Err(err) => NormalizationOutcome::Skipped(err.to_string()),
    }
}

#[cfg(test)]
mod data_path_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn materializes_modelfile_under_data_root_with_spaces() {
        let root = tempfile::tempdir().unwrap();
        let data_dir = root.path().join("cue data with spaces");

        let path = materialize_modelfile_in(&data_dir).unwrap();

        assert_eq!(path, data_dir.join("models/s1/Modelfile"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), S1_MODELFILE);
    }

    #[tokio::test]
    async fn readiness_preserves_probe_failures() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(500).set_body_string("provider-private-body"))
            .mount(&server)
            .await;

        let err = s1_ready(&cue_llm::OllamaAdmin::new(server.uri()))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("500"), "{err}");
        assert!(!err.contains("provider-private-body"), "{err}");
    }

    #[tokio::test]
    async fn readiness_distinguishes_present_and_absent_models() {
        for (models, expected) in [
            (r#"{"models":[{"name":"cue-s1-mini:latest"}]}"#, true),
            (r#"{"models":[{"name":"other-model"}]}"#, false),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/api/tags"))
                .respond_with(ResponseTemplate::new(200).set_body_string(models))
                .expect(1)
                .mount(&server)
                .await;

            assert_eq!(
                s1_ready(&cue_llm::OllamaAdmin::new(server.uri()))
                    .await
                    .unwrap(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn installing_an_existing_model_uses_one_snapshot() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(format!(r#"{{"models":[{{"name":"{}"}}]}}"#, S1_MODEL_NAME)),
            )
            .expect(1)
            .mount(&server)
            .await;

        let message = install_s1(&cue_llm::OllamaAdmin::new(server.uri()))
            .await
            .unwrap();
        assert!(message.contains("already installed"), "{message}");
    }
}
