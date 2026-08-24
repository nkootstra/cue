//! Provisioning of the Python environment for the transcription worker.
//!
//! `cue doctor --fix` creates an isolated venv from a pinned requirements
//! file and installs it into cue's data directory. Ordinary processing
//! never downloads anything: environment setup is always explicit.

use std::path::{Path, PathBuf};

use cue_core::{CueError, PipelineStage, Result};
use tracing::instrument;

/// Pinned dependencies shipped inside the binary.
pub const WORKER_REQUIREMENTS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../workers/faster-whisper/requirements.txt"
));

/// Where the worker venv lives inside the data directory.
pub fn venv_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("venvs").join("faster-whisper")
}

/// Path of the interpreter inside a created venv (POSIX layout).
pub fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python3")
    }
}

/// True when a previously-created venv has a usable interpreter.
pub fn is_provisioned(venv: &Path) -> bool {
    let python = venv_python(venv);
    python.exists() && std::fs::metadata(&python).map(|m| m.len() > 0).unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProvisionAction {
    /// Venv already present; nothing was downloaded.
    AlreadyProvisioned,
    /// A fresh venv was created and dependencies installed.
    Created,
}

/// Create the venv and install pinned requirements into it.
///
/// Idempotent: an existing venv short-circuits unless `force` is set.
#[instrument(skip(base_python))]
pub async fn provision(
    data_dir: &Path,
    base_python: &Path,
    force: bool,
) -> Result<ProvisionAction> {
    let venv = venv_dir(data_dir);
    if is_provisioned(&venv) && !force {
        return Ok(ProvisionAction::AlreadyProvisioned);
    }

    create_venv(base_python, &venv).await?;
    install_requirements(&venv).await?;
    verify_worker(&venv_python(&venv)).await?;
    Ok(ProvisionAction::Created)
}

async fn create_venv(base_python: &Path, venv: &Path) -> Result<()> {
    if let Some(parent) = venv.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| provision_error("could not create venv directory").because(e.to_string()))?;
    }

    let output = tokio::process::Command::new(base_python)
        .args(["-m", "venv"])
        .arg(venv)
        .output()
        .await
        .map_err(|e| {
            provision_error(format!(
                "could not launch {} to create the virtual environment",
                base_python.display()
            ))
            .because(e.to_string())
        })?;

    if !output.status.success() {
        return Err(provision_error("venv creation failed").because(crate::stderr_tail(
            &String::from_utf8_lossy(&output.stderr),
        )));
    }
    Ok(())
}

async fn install_requirements(venv: &Path) -> Result<()> {
    // Materialize the pinned requirements next to the venv so pip reads a
    // real file (portable across platforms).
    let requirements = venv.join("requirements.txt");
    std::fs::write(&requirements, WORKER_REQUIREMENTS)
        .map_err(|e| provision_error("could not write requirements.txt").because(e.to_string()))?;

    let output = tokio::process::Command::new(venv_python(venv))
        .args(["-m", "pip", "install", "--disable-pip-version-check"])
        .arg("--requirement")
        .arg(&requirements)
        .output()
        .await
        .map_err(|e| provision_error("could not run pip in the venv").because(e.to_string()))?;

    if !output.status.success() {
        return Err(provision_error(
            "installing transcription dependencies failed",
        )
        .because(crate::stderr_tail(&String::from_utf8_lossy(&output.stderr)))
        .remedy(
            "check your network connection; pip output above names the \
             failing package",
        ));
    }
    Ok(())
}

/// Confirm the worker imports cleanly inside the fresh venv.
async fn verify_worker(python: &Path) -> Result<()> {
    let script = crate::env::worker_dir().ok_or_else(|| {
        provision_error("could not determine cue's data directory")
            .remedy("set CUE_DATA_DIR to a writable directory")
    })?;
    let script = crate::env::materialize_worker_script(&script)?;

    let output = tokio::process::Command::new(python)
        .arg(&script)
        .arg("--check")
        .output()
        .await
        .map_err(|e| provision_error("could not run worker self-check").because(e.to_string()))?;

    if !output.status.success() {
        return Err(provision_error(
            "the transcription worker failed its self-check after installation",
        )
        .because(crate::stderr_tail(&String::from_utf8_lossy(
            &output.stderr,
        ))));
    }
    Ok(())
}

fn provision_error(summary: impl Into<String>) -> CueError {
    CueError::new(PipelineStage::Transcribe, summary)
        .remedy("run `cue doctor` to inspect the local transcription environment")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unprovisioned_venv_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_provisioned(&dir.path().to_path_buf()));
    }

    #[test]
    fn venv_paths_follow_platform_layout() {
        let venv = PathBuf::from("/data/venvs/faster-whisper");
        if cfg!(windows) {
            assert_eq!(
                venv_python(&venv),
                PathBuf::from("/data/venvs/faster-whisper/Scripts/python.exe")
            );
        } else {
            assert_eq!(
                venv_python(&venv),
                PathBuf::from("/data/venvs/faster-whisper/bin/python3")
            );
        }
    }

    #[test]
    fn requirements_are_pinned() {
        assert!(
            WORKER_REQUIREMENTS.contains("faster-whisper=="),
            "requirements must pin exact versions:\n{WORKER_REQUIREMENTS}"
        );
    }
}
