//! Subtitle policy checks over canonical transcript evidence.

use std::io::BufReader;
use std::path::Path;

use cue_core::{CueError, Result};
use cue_subtitles::{CompiledCue, SubtitlePolicy, TimingRepair, TimingRepairKind};

use crate::cli::SubtitlesCheckArgs;
use crate::render::println_line;

const POLICY_ID: &str = "cue-generic-v1";
const MAX_CHARS_PER_SECOND: f64 = 20.0;

#[derive(serde::Serialize)]
struct SubtitleCheckReport {
    schema_version: u8,
    output: String,
    policy: PolicySnapshot,
    diagnostics: Vec<SubtitleDiagnostic>,
}

#[derive(serde::Serialize)]
struct PolicySnapshot {
    id: &'static str,
    max_lines: usize,
    max_chars_per_line: usize,
    max_duration_ms: u64,
    max_chars_per_second: f64,
}

#[derive(serde::Serialize)]
#[serde(tag = "id")]
enum SubtitleDiagnostic {
    #[serde(rename = "CUE-SUBTITLE-MAX-DURATION")]
    MaxDuration {
        severity: Severity,
        verdict: Verdict,
        word_start: usize,
        word_end: usize,
        start_ms: u64,
        end_ms: u64,
        measured_ms: u64,
        limit_ms: u64,
    },
    #[serde(rename = "CUE-SUBTITLE-LINE-CAPACITY")]
    LineCapacity {
        severity: Severity,
        verdict: Verdict,
        word_start: usize,
        word_end: usize,
        line_count: usize,
        max_line_chars: usize,
        line_limit: usize,
        char_limit: usize,
    },
    #[serde(rename = "CUE-SUBTITLE-READING-SPEED")]
    ReadingSpeed {
        severity: Severity,
        verdict: Verdict,
        word_start: usize,
        word_end: usize,
        start_ms: u64,
        end_ms: u64,
        measured_chars_per_second: f64,
        limit_chars_per_second: f64,
    },
    #[serde(rename = "CUE-SUBTITLE-TIMING-REPAIR")]
    TimingRepair {
        severity: Severity,
        verdict: Verdict,
        word_start: usize,
        word_end: usize,
        kind: TimingRepairName,
        original_start_ms: u64,
        original_end_ms: u64,
        repaired_start_ms: u64,
        repaired_end_ms: u64,
    },
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum Severity {
    Warning,
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum Verdict {
    Violated,
    Repaired,
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum TimingRepairName {
    Shortened,
    Dropped,
}

impl SubtitleDiagnostic {
    fn id(&self) -> &'static str {
        match self {
            Self::MaxDuration { .. } => "CUE-SUBTITLE-MAX-DURATION",
            Self::LineCapacity { .. } => "CUE-SUBTITLE-LINE-CAPACITY",
            Self::ReadingSpeed { .. } => "CUE-SUBTITLE-READING-SPEED",
            Self::TimingRepair { .. } => "CUE-SUBTITLE-TIMING-REPAIR",
        }
    }

    fn message(&self) -> String {
        match self {
            Self::MaxDuration {
                measured_ms,
                limit_ms,
                word_start,
                word_end,
                ..
            } => format!(
                "words {word_start}..{word_end} last {measured_ms} ms (limit {limit_ms} ms)"
            ),
            Self::LineCapacity {
                line_count,
                max_line_chars,
                line_limit,
                char_limit,
                word_start,
                word_end,
                ..
            } => format!(
                "words {word_start}..{word_end} use {line_count} line(s), widest {max_line_chars} chars (limits {line_limit} lines / {char_limit} chars)"
            ),
            Self::ReadingSpeed {
                measured_chars_per_second,
                limit_chars_per_second,
                word_start,
                word_end,
                ..
            } => format!(
                "words {word_start}..{word_end} require {measured_chars_per_second:.1} chars/s (limit {limit_chars_per_second:.1})"
            ),
            Self::TimingRepair {
                kind: TimingRepairName::Shortened,
                original_end_ms,
                repaired_end_ms,
                word_start,
                word_end,
                ..
            } => format!(
                "words {word_start}..{word_end} were shortened to remove an overlap ({original_end_ms} ms -> {repaired_end_ms} ms)"
            ),
            Self::TimingRepair {
                kind: TimingRepairName::Dropped,
                original_start_ms,
                original_end_ms,
                repaired_start_ms,
                repaired_end_ms,
                word_start,
                word_end,
                ..
            } => format!(
                "words {word_start}..{word_end} were dropped because timing had no positive duration ({original_start_ms}..{original_end_ms} ms -> {repaired_start_ms}..{repaired_end_ms} ms)"
            ),
        }
    }
}

pub fn print_help() -> i32 {
    println_line("Usage: cue subtitles check <OUTPUT> [--json]");
    0
}

pub fn check(
    args: SubtitlesCheckArgs,
    corrections: Option<&Path>,
    config: &cue_core::Config,
    output_root: Option<&Path>,
) -> i32 {
    let result = crate::commands::correct::resolve_output_dir_at(&args.output, output_root)
        .and_then(|output_dir| check_output(&output_dir, corrections, config));

    match result {
        Ok(report) => {
            let has_findings = !report.diagnostics.is_empty();
            if args.json {
                match serde_json::to_string_pretty(&report) {
                    Ok(json) => println_line(&json),
                    Err(error) => {
                        eprintln!("could not serialize subtitle report: {error}");
                        return 1;
                    }
                }
            } else if has_findings {
                for diagnostic in &report.diagnostics {
                    println_line(&format!("{}: {}", diagnostic.id(), diagnostic.message()));
                }
                println_line(&format!(
                    "\n{} subtitle issue(s) found.",
                    report.diagnostics.len()
                ));
            } else {
                println_line("No subtitle issues found.");
            }
            i32::from(has_findings)
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn check_output(
    output_dir: &Path,
    corrections: Option<&Path>,
    config: &cue_core::Config,
) -> Result<SubtitleCheckReport> {
    let transcript_path = output_dir.join("transcript.json");
    let transcript_file = std::fs::File::open(&transcript_path).map_err(|error| {
        CueError::general(format!("could not read {}", transcript_path.display()))
            .because(error.to_string())
    })?;
    let mut transcript: cue_core::Transcript =
        serde_json::from_reader(BufReader::new(transcript_file)).map_err(|error| {
            CueError::general(format!("could not parse {}", transcript_path.display()))
                .because(error.to_string())
        })?;
    let word_sources = crate::corrections::CorrectionPlan::prepare(output_dir, corrections)?
        .apply_to_transcript(&mut transcript);
    let policy = SubtitlePolicy {
        max_lines: config.subtitles.max_lines,
        max_chars_per_line: config.subtitles.max_chars_per_line,
        max_duration_ms: config.subtitles.max_duration_ms,
    };
    let compilation = cue_subtitles::compile_with_sources(&transcript, &policy, &word_sources)?;
    let mut diagnostics = Vec::new();
    for compiled in &compilation.cues {
        check_cue(compiled, &policy, &mut diagnostics);
    }
    diagnostics.extend(compilation.repairs.iter().map(repair_diagnostic));

    Ok(SubtitleCheckReport {
        schema_version: 1,
        output: output_dir.to_string_lossy().into_owned(),
        policy: PolicySnapshot {
            id: POLICY_ID,
            max_lines: policy.max_lines,
            max_chars_per_line: policy.max_chars_per_line,
            max_duration_ms: policy.max_duration_ms,
            max_chars_per_second: MAX_CHARS_PER_SECOND,
        },
        diagnostics,
    })
}

fn check_cue(
    compiled: &CompiledCue,
    policy: &SubtitlePolicy,
    diagnostics: &mut Vec<SubtitleDiagnostic>,
) {
    let cue = &compiled.cue;
    let source = compiled.source;
    let duration_ms = cue.end_ms.saturating_sub(cue.start_ms);
    if duration_ms > policy.max_duration_ms {
        diagnostics.push(SubtitleDiagnostic::MaxDuration {
            severity: Severity::Warning,
            verdict: Verdict::Violated,
            word_start: source.word_start,
            word_end: source.word_end,
            start_ms: cue.start_ms,
            end_ms: cue.end_ms,
            measured_ms: duration_ms,
            limit_ms: policy.max_duration_ms,
        });
    }

    let line_count = cue.text.lines().count();
    let max_line_chars = cue
        .text
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    if line_count > policy.max_lines || max_line_chars > policy.max_chars_per_line {
        diagnostics.push(SubtitleDiagnostic::LineCapacity {
            severity: Severity::Warning,
            verdict: Verdict::Violated,
            word_start: source.word_start,
            word_end: source.word_end,
            line_count,
            max_line_chars,
            line_limit: policy.max_lines,
            char_limit: policy.max_chars_per_line,
        });
    }

    if duration_ms > 0 {
        let character_count = cue
            .text
            .chars()
            .filter(|character| *character != '\n')
            .count();
        let chars_per_second = character_count as f64 * 1_000.0 / duration_ms as f64;
        if chars_per_second > MAX_CHARS_PER_SECOND {
            diagnostics.push(SubtitleDiagnostic::ReadingSpeed {
                severity: Severity::Warning,
                verdict: Verdict::Violated,
                word_start: source.word_start,
                word_end: source.word_end,
                start_ms: cue.start_ms,
                end_ms: cue.end_ms,
                measured_chars_per_second: chars_per_second,
                limit_chars_per_second: MAX_CHARS_PER_SECOND,
            });
        }
    }
}

fn repair_diagnostic(repair: &TimingRepair) -> SubtitleDiagnostic {
    SubtitleDiagnostic::TimingRepair {
        severity: Severity::Warning,
        verdict: Verdict::Repaired,
        word_start: repair.source.word_start,
        word_end: repair.source.word_end,
        kind: match repair.kind {
            TimingRepairKind::Shortened => TimingRepairName::Shortened,
            TimingRepairKind::Dropped => TimingRepairName::Dropped,
        },
        original_start_ms: repair.start_ms,
        original_end_ms: repair.original_end_ms,
        repaired_start_ms: repair.start_ms,
        repaired_end_ms: repair.repaired_end_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::{Severity, SubtitleDiagnostic, TimingRepairName, Verdict};

    #[test]
    fn serialized_diagnostic_ids_match_human_ids() {
        let diagnostics = [
            SubtitleDiagnostic::MaxDuration {
                severity: Severity::Warning,
                verdict: Verdict::Violated,
                word_start: 0,
                word_end: 1,
                start_ms: 0,
                end_ms: 1,
                measured_ms: 1,
                limit_ms: 1,
            },
            SubtitleDiagnostic::LineCapacity {
                severity: Severity::Warning,
                verdict: Verdict::Violated,
                word_start: 0,
                word_end: 1,
                line_count: 1,
                max_line_chars: 1,
                line_limit: 1,
                char_limit: 1,
            },
            SubtitleDiagnostic::ReadingSpeed {
                severity: Severity::Warning,
                verdict: Verdict::Violated,
                word_start: 0,
                word_end: 1,
                start_ms: 0,
                end_ms: 1,
                measured_chars_per_second: 1.0,
                limit_chars_per_second: 1.0,
            },
            SubtitleDiagnostic::TimingRepair {
                severity: Severity::Warning,
                verdict: Verdict::Repaired,
                word_start: 0,
                word_end: 1,
                kind: TimingRepairName::Dropped,
                original_start_ms: 0,
                original_end_ms: 1,
                repaired_start_ms: 0,
                repaired_end_ms: 0,
            },
        ];

        for diagnostic in diagnostics {
            let id = diagnostic.id();
            let serialized = serde_json::to_value(diagnostic).unwrap();
            assert_eq!(serialized["id"], id);
        }
    }
}
