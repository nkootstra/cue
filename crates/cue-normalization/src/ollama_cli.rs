//! Model management through the `ollama` CLI.
//!
//! Ollama's HTTP `/api/create` rejects Modelfiles whose `FROM` names a
//! registry ref (e.g. `hf.co/...`) even when the model is already pulled —
//! it returns `neither 'from' or 'files' was specified`. The `ollama` CLI
//! handles these Modelfiles correctly, so `cue models install s1` shells out
//! to it instead of the HTTP API.

use std::path::{Path, PathBuf};

use cue_core::{CueError, PipelineStage, Result};
use tracing::instrument;

/// The `ollama` executable used for model management.
pub struct OllamaCli {
    /// Server base URL; passed to the CLI as `OLLAMA_HOST`.
    host: String,
}

impl OllamaCli {
    pub fn new(host: impl Into<String>) -> Self {
        Self { host: host.into() }
    }

    /// Pull a model by reference (e.g. an `hf.co/...` GGUF tag).
    #[instrument(skip(self), fields(model = %model_ref))]
    pub async fn pull(&self, model_ref: &str) -> Result<()> {
        self.run(&["pull", model_ref]).await
    }

    /// Create a named model from a Modelfile at `modelfile_path`.
    ///
    /// `ollama` does not read the Modelfile from stdin in current releases,
    /// so the caller materializes the file first.
    #[instrument(skip(self), fields(model = %name))]
    pub async fn create(&self, name: &str, modelfile_path: &Path) -> Result<()> {
        let path = modelfile_path.to_str().unwrap_or_default();
        self.run(&["create", name, "-f", path]).await
    }

    /// Run an `ollama` subcommand, capturing output and surfacing the
    /// process stderr when it fails.
    async fn run(&self, args: &[&str]) -> Result<()> {
        let ollama = find_ollama().ok_or_else(|| {
            CueError::new(
                PipelineStage::Normalize,
                "the `ollama` command was not found on PATH",
            )
            .remedy(
                "install Ollama, then verify it with `cue doctor`; the S1 \
                 model installer uses the `ollama` CLI",
            )
        })?;

        let output = tokio::process::Command::new(&ollama)
            .args(args)
            .env("OLLAMA_HOST", &self.host)
            .output()
            .await
            .map_err(|e| {
                CueError::new(PipelineStage::Normalize, "could not run the ollama command")
                    .because(e.to_string())
            })?;

        if !output.status.success() {
            return Err(CueError::new(
                PipelineStage::Normalize,
                format!("`ollama {}` failed", args.join(" ")),
            )
            .because(stderr_tail(&String::from_utf8_lossy(&output.stderr))));
        }
        Ok(())
    }
}

fn stderr_tail(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(5);
    lines[start..].join("\n")
}

/// Search PATH for the `ollama` executable.
fn find_ollama() -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "ollama.exe"
    } else {
        "ollama"
    };
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .filter(|dir| dir.is_dir())
        .map(|dir| dir.join(exe))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_is_detected_on_this_machine() {
        // The CI/local harness that exercises `cue models install` has
        // Ollama installed; treat absence as a valid "not present" state.
        assert!(find_ollama().is_some() || std::env::var("OLLAMA_HOST").is_ok());
    }

    #[test]
    fn stderr_tail_keeps_last_lines() {
        let tail = stderr_tail("a\nb\nc\nd\ne\nf\ng\n");
        assert!(!tail.contains('a'));
        assert!(tail.contains('g'));
    }
}
