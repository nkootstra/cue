//! Provisioning of the Python environment for the transcription worker.
//!
//! `cue doctor --fix` creates an isolated venv from a pinned requirements
//! file and installs it into cue's data directory. Ordinary processing
//! never downloads anything: environment setup is always explicit.

use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cue_core::{CueError, PipelineStage, Result};
use tracing::instrument;

/// Pinned dependencies shipped inside the binary.
pub const WORKER_REQUIREMENTS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../workers/faster-whisper/requirements.txt"
));

const PROVISION_RECEIPT_FILE: &str = ".cue-provisioned";
const PROVISION_RECEIPT_VERSION: &str = "cue-faster-whisper-provision/v1\n";
static RECEIPT_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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

/// True when a previously-created venv has a usable interpreter and was
/// fully provisioned for the requirements embedded in this binary.
pub fn is_provisioned(venv: &Path) -> bool {
    let python = venv_python(venv);
    let interpreter_is_usable = std::fs::metadata(&python)
        .map(|metadata| metadata.is_file() && metadata.len() > 0)
        .unwrap_or(false);
    interpreter_is_usable
        && std::fs::read(receipt_path(venv))
            .map(|receipt| receipt == expected_receipt())
            .unwrap_or(false)
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

    begin_provisioning(&venv)?;
    let result = async {
        create_venv(base_python, &venv).await?;
        install_requirements(&venv).await?;
        verify_worker(&venv_python(&venv), data_dir).await
    }
    .await;
    complete_provisioning(&venv, result)?;
    Ok(ProvisionAction::Created)
}

fn receipt_path(venv: &Path) -> PathBuf {
    venv.join(PROVISION_RECEIPT_FILE)
}

fn expected_receipt() -> Vec<u8> {
    let mut receipt =
        Vec::with_capacity(PROVISION_RECEIPT_VERSION.len() + WORKER_REQUIREMENTS.len());
    receipt.extend_from_slice(PROVISION_RECEIPT_VERSION.as_bytes());
    receipt.extend_from_slice(WORKER_REQUIREMENTS.as_bytes());
    receipt
}

fn begin_provisioning(venv: &Path) -> Result<()> {
    match std::fs::remove_file(receipt_path(venv)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(provision_error(
            "could not invalidate the previous provisioning receipt",
        )
        .because(error.to_string())),
    }
}

fn complete_provisioning(venv: &Path, result: Result<()>) -> Result<()> {
    result?;
    write_receipt_atomic(venv)
}

fn write_receipt_atomic(venv: &Path) -> Result<()> {
    std::fs::create_dir_all(venv).map_err(|error| {
        provision_error("could not create the provisioned environment directory")
            .because(error.to_string())
    })?;

    let receipt = receipt_path(venv);
    loop {
        let sequence = RECEIPT_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = venv.join(format!(
            ".{PROVISION_RECEIPT_FILE}.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(provision_error("could not create provisioning receipt")
                    .because(error.to_string()));
            }
        };

        if let Err(error) = file
            .write_all(&expected_receipt())
            .and_then(|()| file.sync_all())
        {
            drop(file);
            let _ = std::fs::remove_file(&temp);
            return Err(
                provision_error("could not write provisioning receipt").because(error.to_string())
            );
        }
        drop(file);

        match std::fs::rename(&temp, &receipt) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == ErrorKind::AlreadyExists && is_provisioned(venv) => {
                let _ = std::fs::remove_file(temp);
                return Ok(());
            }
            Err(error) => {
                let _ = std::fs::remove_file(temp);
                return Err(provision_error("could not publish provisioning receipt")
                    .because(error.to_string()));
            }
        }
    }
}

async fn create_venv(base_python: &Path, venv: &Path) -> Result<()> {
    if let Some(parent) = venv.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            provision_error("could not create venv directory").because(e.to_string())
        })?;
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
        return Err(provision_error("venv creation failed")
            .because(crate::stderr_tail(&String::from_utf8_lossy(&output.stderr))));
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
        return Err(
            provision_error("installing transcription dependencies failed")
                .because(crate::stderr_tail(&String::from_utf8_lossy(&output.stderr)))
                .remedy(
                    "check your network connection; pip output above names the \
             failing package",
                ),
        );
    }
    Ok(())
}

/// Confirm the worker imports cleanly inside the fresh venv.
async fn verify_worker(python: &Path, data_dir: &Path) -> Result<()> {
    let script = crate::env::worker_dir_in(data_dir);
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
        .because(crate::stderr_tail(&String::from_utf8_lossy(&output.stderr))));
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
        assert!(!is_provisioned(dir.path()));
    }

    fn create_fake_interpreter(venv: &Path) {
        let python = venv_python(venv);
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::fs::write(python, b"fake interpreter").unwrap();
    }

    #[test]
    fn interpreter_without_receipt_is_not_provisioned() {
        let dir = tempfile::tempdir().unwrap();
        create_fake_interpreter(dir.path());

        assert!(!is_provisioned(dir.path()));
    }

    #[test]
    fn matching_receipt_marks_environment_provisioned() {
        let dir = tempfile::tempdir().unwrap();
        create_fake_interpreter(dir.path());
        write_receipt_atomic(dir.path()).unwrap();

        assert!(is_provisioned(dir.path()));
    }

    #[test]
    fn mismatched_receipt_requires_reprovisioning() {
        let dir = tempfile::tempdir().unwrap();
        create_fake_interpreter(dir.path());
        std::fs::write(receipt_path(dir.path()), b"stale fingerprint").unwrap();

        assert!(!is_provisioned(dir.path()));
    }

    #[test]
    fn failed_workflow_leaves_no_receipt_and_can_retry() {
        let dir = tempfile::tempdir().unwrap();
        create_fake_interpreter(dir.path());
        write_receipt_atomic(dir.path()).unwrap();
        assert!(is_provisioned(dir.path()));

        begin_provisioning(dir.path()).unwrap();
        let result = complete_provisioning(dir.path(), Err(provision_error("simulated failure")));
        assert!(result.is_err());
        assert!(!receipt_path(dir.path()).exists());

        complete_provisioning(dir.path(), Ok(())).unwrap();
        assert!(is_provisioned(dir.path()));
    }

    #[test]
    fn receipt_paths_support_spaces() {
        let root = tempfile::tempdir().unwrap();
        let venv = root.path().join("cue data").join("venv with spaces");
        create_fake_interpreter(&venv);
        write_receipt_atomic(&venv).unwrap();

        assert!(is_provisioned(&venv));
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
