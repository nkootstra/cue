//! Resolution of the Python environment that runs the worker.
//!
//! The worker script is embedded in the binary and materialized into cue's
//! data directory, so the CLI stays self-contained. The Python interpreter
//! itself comes from an auto-provisioned venv when present (created by a
//! later `cue doctor --fix`), falling back to `python3` on PATH.

use std::path::{Path, PathBuf};

use cue_core::{CueError, PipelineStage, Result};

pub use cue_core::paths::data_dir;

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

/// Directory holding the worker script inside the data dir.
pub fn worker_dir() -> Option<PathBuf> {
    data_dir().map(|dir| worker_dir_in(&dir))
}

pub(crate) fn worker_dir_in(data_dir: &Path) -> PathBuf {
    data_dir.join("workers").join("faster-whisper")
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
    let data_dir = data_dir();
    let python = match python_override {
        Some(explicit) => explicit.to_path_buf(),
        None => find_python(data_dir.as_deref()).ok_or_else(|| {
            CueError::new(PipelineStage::Transcribe, "no usable Python interpreter")
                .because("python3 was not found on PATH and no override was configured")
                .remedy("install Python 3.10+ and verify with `cue doctor`")
        })?,
    };

    // Escape hatch for development and testing: point cue at an
    // alternative worker implementation.
    if let Some(script) = std::env::var_os("CUE_FASTER_WHISPER_SCRIPT") {
        return Ok(WorkerEnvironment {
            python,
            script: PathBuf::from(script),
        });
    }

    let dir = data_dir.as_deref().map(worker_dir_in).ok_or_else(|| {
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
fn find_python(data_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(data_dir) = data_dir {
        let venv = crate::provision::venv_dir(data_dir);
        if crate::provision::is_provisioned(&venv) {
            return Some(crate::provision::venv_python(&venv));
        }
    }
    // Cheap PATH search without spawning anything: reuse cue-media's helper.
    crate::find_binary_on_path("python3")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materializes_embedded_script() {
        let dir = tempfile::tempdir().unwrap();
        let path = materialize_worker_script(dir.path()).unwrap();
        assert_eq!(path.file_name().unwrap(), "cue_faster_whisper.py");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), WORKER_SCRIPT);
    }

    #[test]
    fn rewrites_stale_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let path = materialize_worker_script(dir.path()).unwrap();
        std::fs::write(&path, "# stale version").unwrap();

        materialize_worker_script(dir.path()).unwrap();
        // Restored to the embedded version.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), WORKER_SCRIPT);
    }

    #[test]
    fn skips_rewrite_when_current() {
        let dir = tempfile::tempdir().unwrap();
        let path = materialize_worker_script(dir.path()).unwrap();
        let before = std::fs::metadata(&path).unwrap().len();
        std::thread::sleep(std::time::Duration::from_millis(10));
        materialize_worker_script(dir.path()).unwrap();
        // Content untouched (mtime may not move, so compare bytes).
        assert_eq!(std::fs::read_to_string(&path).unwrap(), WORKER_SCRIPT);
        assert_eq!(before, std::fs::metadata(&path).unwrap().len());
    }

    #[test]
    fn command_prefix_is_interpreter_then_script() {
        let env = WorkerEnvironment {
            python: PathBuf::from("/usr/bin/python3"),
            script: PathBuf::from("/data/worker.py"),
        };
        assert_eq!(
            env.command_prefix(),
            vec!["/usr/bin/python3", "/data/worker.py"]
        );
    }
}
