//! Rendering helpers shared by commands.
//!
//! User-facing output goes to stdout; diagnostics go through `tracing`
//! (stderr). This module is deliberately dumb text so it stays portable.

use std::io::Write as _;

use cue_media::{ToolReport, ToolStatus};

/// Render one tool report as a fixed-width status line.
pub fn tool_line(report: &ToolReport) -> String {
    let label = format!("{:<10}", report.name);
    match &report.status {
        ToolStatus::Available { path, version } => {
            format!("{label} ok       {path} ({version})")
        }
        ToolStatus::Missing => format!(
            "{label} missing  not found on PATH"
        ),
        ToolStatus::Error(reason) => {
            format!("{label} broken   {reason}")
        }
    }
}

pub fn println_line(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{text}");
}

/// Format a duration in milliseconds for humans ("1m 32s", "4.5s").
pub fn human_duration(ms: u64) -> String {
    let total_seconds = ms / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    let tenths = ms % 1000 / 100;
    if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else if ms >= 1000 {
        format!("{total_seconds}.{tenths}s")
    } else {
        format!("{ms}ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(status: ToolStatus) -> ToolReport {
        ToolReport {
            name: "FFmpeg".into(),
            status,
        }
    }

    #[test]
    fn available_lines_include_path_and_version() {
        let line = tool_line(&report(ToolStatus::Available {
            path: "/usr/bin/ffmpeg".into(),
            version: "ffmpeg version 8.0.1".into(),
        }));
        assert!(line.contains("ok"), "{line}");
        assert!(line.contains("/usr/bin/ffmpeg"), "{line}");
        assert!(line.contains("8.0.1"), "{line}");
    }

    #[test]
    fn missing_and_broken_lines_explain_themselves() {
        let missing = tool_line(&report(ToolStatus::Missing));
        assert!(missing.contains("missing"), "{missing}");
        assert!(missing.contains("PATH"), "{missing}");

        let broken = tool_line(&report(ToolStatus::Error("exit 1".into())));
        assert!(broken.contains("broken"), "{broken}");
        assert!(broken.contains("exit 1"), "{broken}");
    }

    #[test]
    fn durations_render_for_humans() {
        assert_eq!(human_duration(500), "500ms");
        assert_eq!(human_duration(4_500), "4.5s");
        assert_eq!(human_duration(92_000), "1m 32s");
    }
}
