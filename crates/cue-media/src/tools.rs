//! Detection of external tools the pipeline depends on.
//!
//! Each check reports where a tool was found and its version string, so
//! `cue doctor` can explain exactly what is missing or broken.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::ToolStatus;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolReport {
    /// Human-facing tool name ("FFmpeg", "Python", ...).
    pub name: String,
    pub status: ToolStatus,
}

impl ToolReport {
    pub fn is_ok(&self) -> bool {
        matches!(self.status, ToolStatus::Available { .. })
    }
}

/// Search PATH for an executable name.
///
/// Returns the first directory containing a file with that name that is
/// executable. On Windows also considers `.exe` suffixes.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exe_names = candidate_executable_names(name);
    std::env::split_paths(&path_var)
        .filter(|dir| dir.is_dir())
        .find_map(|dir| {
            for exe in &exe_names {
                let candidate = dir.join(exe);
                if is_executable(&candidate) {
                    return Some(candidate);
                }
            }
            None
        })
}

fn candidate_executable_names(name: &str) -> Vec<String> {
    if cfg!(windows) && !name.to_ascii_lowercase().ends_with(".exe") {
        vec![format!("{name}.exe"), name.to_string()]
    } else {
        vec![name.to_string()]
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

/// Run `<binary> <args>` and return its first stdout line.
async fn first_output_line(binary: &Path, args: &[&str]) -> Result<String, String> {
    let output = tokio::process::Command::new(binary)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("could not execute {}: {e}", binary.display()))?;

    if !output.status.success() {
        return Err(format!(
            "exited with {}",
            output.status.code().unwrap_or(-1)
        ));
    }

    String::from_utf8(output.stdout)
        .map_err(|_| "output was not valid UTF-8".to_string())
        .map(|text| text.lines().next().unwrap_or_default().to_string())
}

/// Probe a tool by running it with version arguments.
pub async fn probe_version(binary: &Path, args: &[&str]) -> Result<String, String> {
    first_output_line(binary, args).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_real_binary_on_path() {
        // The test environment is expected to have at least one of these.
        let found = ["cargo", "ffmpeg", "python3"]
            .iter()
            .any(|name| find_on_path(name).is_some());
        assert!(found, "no known binaries found on PATH");
    }

    #[test]
    fn missing_binary_is_none() {
        assert_eq!(find_on_path("definitely-not-a-real-binary-xyz"), None);
    }

    #[tokio::test]
    async fn probe_returns_first_line() {
        // /bin/echo exists on macOS and Linux CI.
        if !Path::new("/bin/echo").exists() {
            return;
        }
        let line = probe_version(Path::new("/bin/echo"), &["hello", "world"])
            .await
            .unwrap();
        assert_eq!(line.trim(), "hello world");
    }

    #[tokio::test]
    async fn probe_failing_command_is_error() {
        // `false` exits non-zero without output.
        let binary = if Path::new("/usr/bin/false").exists() {
            PathBuf::from("/usr/bin/false")
        } else {
            PathBuf::from("/bin/false")
        };
        if !binary.exists() {
            return;
        }
        let result = probe_version(&binary, &[]).await;
        assert!(result.is_err());
    }
}
