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
    let report = check_tool(&PYTHON).await;
    ToolReport {
        name: report.name,
        status: classify_python_status(report.status),
    }
}

fn classify_python_status(status: ToolStatus) -> ToolStatus {
    let ToolStatus::Available { path, version } = status else {
        return status;
    };

    let Some((major, minor)) = parse_python_version(&version) else {
        return ToolStatus::Error(format!(
            "could not parse Python version banner: {version:?}"
        ));
    };

    if major < 3 || (major == 3 && minor < PYTHON_MIN_MINOR) {
        return ToolStatus::Error(format!(
            "Python {major}.{minor} found, but the transcription worker needs >= 3.{PYTHON_MIN_MINOR}"
        ));
    }

    ToolStatus::Available { path, version }
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
    let (ffmpeg, ffprobe, python) =
        tokio::join!(check_tool(&FFMPEG), check_tool(&FFPROBE), check_python());
    Environment {
        reports: vec![ffmpeg, ffprobe, python],
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

    #[test]
    fn classifies_supported_and_unsupported_python_versions() {
        for version in ["Python 3.10.0", "Python 3.14.5", "Python 4.0.0"] {
            let status = ToolStatus::Available {
                path: "/usr/bin/python3".into(),
                version: version.into(),
            };
            assert!(matches!(
                classify_python_status(status),
                ToolStatus::Available { .. }
            ));
        }

        for version in ["Python 2.7.18", "Python 3.9.19"] {
            let status = ToolStatus::Available {
                path: "/usr/bin/python3".into(),
                version: version.into(),
            };
            assert!(matches!(
                classify_python_status(status),
                ToolStatus::Error(reason) if reason.contains("needs >= 3.10")
            ));
        }
    }

    #[test]
    fn classifies_malformed_banner_and_preserves_probe_errors() {
        let malformed = ToolStatus::Available {
            path: "/usr/bin/python3".into(),
            version: "not a Python banner".into(),
        };
        assert!(matches!(
            classify_python_status(malformed),
            ToolStatus::Error(reason) if reason.contains("could not parse")
        ));

        let error = ToolStatus::Error("permission denied".into());
        assert_eq!(
            classify_python_status(error),
            ToolStatus::Error("permission denied".into())
        );
        assert_eq!(
            classify_python_status(ToolStatus::Missing),
            ToolStatus::Missing
        );
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
