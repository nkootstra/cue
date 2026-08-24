//! Named checks for the tools cue depends on, consumed by `cue doctor`.

use super::ToolStatus;
use super::tools::{find_on_path, probe_version};
use crate::ToolReport;

/// A named external tool and how to ask it for a version banner.
pub struct ToolSpec {
    /// Name used for PATH lookup.
    pub binary: &'static str,
    /// Human-facing name in doctor output.
    pub label: &'static str,
    /// Arguments that make the tool print its version.
    pub version_args: &'static [&'static str],
}

pub const FFMPEG: ToolSpec = ToolSpec {
    binary: "ffmpeg",
    label: "FFmpeg",
    version_args: &["-version"],
};

pub const FFPROBE: ToolSpec = ToolSpec {
    binary: "ffprobe",
    label: "FFprobe",
    version_args: &["-version"],
};

pub const PYTHON: ToolSpec = ToolSpec {
    binary: "python3",
    label: "Python",
    version_args: &["--version"],
};

/// Probe one tool spec against the current system.
pub async fn check_tool(spec: &ToolSpec) -> ToolReport {
    let report = match find_on_path(spec.binary) {
        None => ToolStatus::Missing,
        Some(path) => match probe_version(&path, spec.version_args).await {
            Ok(version) => ToolStatus::Available {
                path: path.display().to_string(),
                version,
            },
            Err(reason) => ToolStatus::Error(reason),
        },
    };
    ToolReport {
        name: spec.label.to_string(),
        status: report,
    }
}

/// Python needs a minimum minor version for the faster-whisper worker.
///
/// The worker targets modern CPython; 3.10 is the oldest line
/// faster-whisper's dependencies still support comfortably.
const PYTHON_MIN_MINOR: u32 = 10;

pub async fn check_python() -> ToolReport {
    let mut report = check_tool(&PYTHON).await;
    if let Some(path) = find_on_path(PYTHON.binary)
        && let Ok(version) = probe_version(&path, PYTHON.version_args).await
        && let Some(parsed) = parse_python_version(&version)
        && parsed.1 < PYTHON_MIN_MINOR
    {
        report.status = ToolStatus::Error(format!(
            "Python {}.{} found, but the transcription worker needs >= 3.{PYTHON_MIN_MINOR}",
            parsed.0, parsed.1
        ));
    }
    report
}

/// Parse "Python 3.14.5" into (3, 14).
fn parse_python_version(banner: &str) -> Option<(u32, u32)> {
    let rest = banner.strip_prefix("Python")?;
    let mut parts = rest.trim().split('.');
    let major = parts.next()?.trim().parse().ok()?;
    let minor = parts.next()?.trim().parse().ok()?;
    Some((major, minor))
}

/// Every environment check `cue doctor` reports, in display order.
#[derive(Debug, Clone)]
pub struct Environment {
    pub reports: Vec<ToolReport>,
}

impl Environment {
    pub fn all_ok(&self) -> bool {
        self.reports.iter().all(|r| r.is_ok())
    }

    pub fn ffmpeg(&self) -> Option<&ToolReport> {
        self.reports.iter().find(|r| r.name == FFMPEG.label)
    }

    pub fn ffprobe(&self) -> Option<&ToolReport> {
        self.reports.iter().find(|r| r.name == FFPROBE.label)
    }

    pub fn python(&self) -> Option<&ToolReport> {
        self.reports.iter().find(|r| r.name == PYTHON.label)
    }
}

pub async fn check_environment() -> Environment {
    Environment {
        reports: vec![
            check_tool(&FFMPEG).await,
            check_tool(&FFPROBE).await,
            check_python().await,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_banners() {
        assert_eq!(parse_python_version("Python 3.14.5"), Some((3, 14)));
        assert_eq!(parse_python_version("Python 2.7.18"), Some((2, 7)));
        assert_eq!(parse_python_version("not python"), None);
        assert_eq!(parse_python_version("Python"), None);
    }

    #[tokio::test]
    async fn environment_reports_all_tools() {
        let env = check_environment().await;
        let names: Vec<_> = env.reports.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["FFmpeg", "FFprobe", "Python"]);
        // Every report has a name and a definite status.
        for report in &env.reports {
            match &report.status {
                ToolStatus::Available { path, version } => {
                    assert!(!path.is_empty(), "{}: empty path", report.name);
                    assert!(!version.is_empty(), "{}: empty version", report.name);
                }
                ToolStatus::Missing | ToolStatus::Error(_) => {}
            }
        }
    }

    #[tokio::test]
    async fn ffmpeg_check_reports_available_or_missing() {
        // This machine either has ffmpeg or it does not; both are valid
        // outcomes, but the status must be one of the two (never an error,
        // since `-version` always succeeds on a working binary).
        let report = check_tool(&FFMPEG).await;
        assert!(matches!(
            report.status,
            ToolStatus::Available { .. } | ToolStatus::Missing
        ));
    }

    #[tokio::test]
    async fn missing_tool_spec_reports_missing() {
        let bogus = ToolSpec {
            binary: "definitely-not-real-xyz",
            label: "Bogus",
            version_args: &[],
        };
        let report = check_tool(&bogus).await;
        assert_eq!(report.status, ToolStatus::Missing);
        assert!(!report.is_ok());
    }

    #[tokio::test]
    async fn all_ok_requires_every_report_available() {
        let env = Environment {
            reports: vec![ToolReport {
                name: "A".into(),
                status: ToolStatus::Available {
                    path: "/bin/a".into(),
                    version: "1".into(),
                },
            }],
        };
        assert!(env.all_ok());

        let broken = Environment {
            reports: vec![
                ToolReport {
                    name: "A".into(),
                    status: ToolStatus::Available {
                        path: "/bin/a".into(),
                        version: "1".into(),
                    },
                },
                ToolReport {
                    name: "B".into(),
                    status: ToolStatus::Missing,
                },
            ],
        };
        assert!(!broken.all_ok());
    }
}
