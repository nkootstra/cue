//! Resolution of the Python environment that runs the worker.
//!
//! The worker script is embedded in the binary and materialized into cue's
//! data directory, so the CLI stays self-contained. The Python interpreter
//! itself comes from an auto-provisioned venv when present (created by a
//! later `cue doctor --fix`), falling back to `python3` on PATH.

use std::path::{Path, PathBuf};

use cue_core::{CueError, PipelineStage, Result};

/// The embedded worker script text.
pub const WORKER_SCRIPT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../workers/faster-whisper/src/cue_faster_whisper.py"
));

/// Everything needed to launch the worker once.
#[derive(Debug, Clone)]
pub struct WorkerEnvironment {
    /// Interpreter used to run the script.
    pub python: PathBuf,
    /// Materialized worker script path.
    pub script: PathBuf,
}

impl WorkerEnvironment {
    pub fn command_prefix(&self) -> Vec<String> {
        vec![
            self.python.display().to_string(),
            self.script.display().to_string(),
        ]
    }
}

/// Cue's data directory: `$CUE_DATA_DIR`, else `$XDG_DATA_HOME/cue`, else
/// `~/.local/share/cue`.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CUE_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".local/share")))?;
    Some(base.join("cue"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Directory holding the worker script inside the data dir.
pub fn worker_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("workers").join("faster-whisper"))
}

/// Write the embedded script into place when missing or stale.
///
/// Comparing content (not timestamps) keeps reinstalls cheap while still
/// propagating upgrades to users who never delete their data dir.
pub fn materialize_worker_script(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).map_err(|e| {
        CueError::new(
            PipelineStage::Transcribe,
            format!("could not create worker directory {}", dir.display()),
        )
        .because(e.to_string())
    })?;

    let path = dir.join("cue_faster_whisper.py");
    let stale = std::fs::read_to_string(&path)
        .map(|existing| existing != WORKER_SCRIPT)
        .unwrap_or(true);

    if stale {
        std::fs::write(&path, WORKER_SCRIPT).map_err(|e| {
            CueError::new(
                PipelineStage::Transcribe,
                format!("could not write worker script to {}", path.display()),
            )
            .because(e.to_string())
        })?;
    }

    Ok(path)
}

/// Locate or materialize a runnable worker environment.
///
/// `python_override` wins over discovery; discovery prefers the
/// auto-provisioned venv, then falls back to `python3`.
pub fn resolve(python_override: Option<&Path>) -> Result<WorkerEnvironment> {
    let python = match python_override {
        Some(explicit) => explicit.to_path_buf(),
        None => find_python().ok_or_else(|| {
            CueError::new(PipelineStage::Transcribe, "no usable Python interpreter")
                .because("python3 was not found on PATH and no override was configured")
                .remedy("install Python 3.10+ and verify with `cue doctor`")
        })?,
    };

    let dir = worker_dir().ok_or_else(|| {
        CueError::new(
            PipelineStage::Transcribe,
            "could not determine cue's data directory",
        )
        .remedy("set CUE_DATA_DIR to a writable directory")
    })?;

    let script = materialize_worker_script(&dir)?;
    Ok(WorkerEnvironment { python, script })
}

/// Prefer the venv provisioned for cue; fall back to system python3.
fn find_python() -> Option<PathBuf> {
    if let Some(dir) = data_dir() {
        let venv_python = dir
            .join("venvs")
            .join("faster-whisper")
            .join("bin")
            .join("python3");
        if venv_python.exists() {
            return Some(venv_python);
        }
    }
    // Cheap PATH search without spawning anything: reuse cue-media's helper.
    crate::find_binary_on_path("python3")
}
