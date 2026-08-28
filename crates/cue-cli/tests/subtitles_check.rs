use std::fs;
use std::process::Command;

use cue_core::{Segment, Transcript, Word};

fn cue_command(config_dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cue"));
    command.env("XDG_CONFIG_HOME", config_dir);
    command
}

fn word(text: impl Into<String>, start_ms: u64, end_ms: u64) -> Word {
    Word {
        text: text.into(),
        start_ms,
        end_ms,
        confidence: Some(0.9),
        speaker: None,
    }
}

fn write_transcript(output: &std::path::Path, words: Vec<Word>, duration_ms: u64) {
    let transcript = Transcript {
        schema_version: cue_core::TRANSCRIPT_SCHEMA_VERSION,
        language: "en".into(),
        duration_ms,
        segments: vec![Segment {
            start_ms: words.first().map_or(0, |word| word.start_ms),
            end_ms: words.last().map_or(0, |word| word.end_ms),
            text: words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            word_start: 0,
            word_end: words.len(),
        }],
        words,
    };
    write_transcript_value(output, &transcript);
}

fn write_transcript_value(output: &std::path::Path, transcript: &Transcript) {
    fs::create_dir_all(output).unwrap();
    fs::write(
        output.join("transcript.json"),
        serde_json::to_vec_pretty(transcript).unwrap(),
    )
    .unwrap();
}

#[test]
fn subtitle_check_json_reports_source_linked_policy_violations_and_repairs() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    let long_word = format!("{}.", "a".repeat(200));
    write_transcript(
        &output,
        vec![word(long_word, 0, 8_000), word("next.", 7_000, 9_000)],
        9_000,
    );

    let result = cue_command(temp.path())
        .args(["subtitles", "check", output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["policy"]["id"], "cue-generic-v1");
    let diagnostics = report["diagnostics"].as_array().unwrap();
    for id in [
        "CUE-SUBTITLE-MAX-DURATION",
        "CUE-SUBTITLE-LINE-CAPACITY",
        "CUE-SUBTITLE-READING-SPEED",
        "CUE-SUBTITLE-TIMING-REPAIR",
    ] {
        assert!(
            diagnostics.iter().any(|item| item["id"] == id),
            "missing {id}: {diagnostics:?}"
        );
    }
    assert!(diagnostics.iter().all(|item| {
        item["word_start"].is_number()
            && item["word_end"].is_number()
            && item["severity"] == "warning"
    }));
    assert!(diagnostics.iter().any(|item| {
        item["id"] == "CUE-SUBTITLE-TIMING-REPAIR"
            && item["verdict"] == "repaired"
            && item["kind"] == "shortened"
            && item["original_end_ms"] == 8000
            && item["repaired_end_ms"] == 7000
    }));
}

#[test]
fn subtitle_check_human_output_is_clean_when_cues_meet_policy() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    write_transcript(
        &output,
        vec![word("hello", 0, 400), word("world.", 410, 1_000)],
        1_000,
    );

    let result = cue_command(temp.path())
        .args(["subtitles", "check", output.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "No subtitle issues found.\n"
    );
}

#[test]
fn subtitle_check_json_is_a_successful_empty_report_when_cues_meet_policy() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    write_transcript(&output, vec![word("hello.", 0, 1_000)], 1_000);

    let result = cue_command(temp.path())
        .args(["subtitles", "check", output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert!(result.status.success());
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["diagnostics"], serde_json::json!([]));
}

#[test]
fn subtitle_check_uses_the_correction_plan_that_renders_subtitles() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    write_transcript(&output, vec![word("short.", 0, 1_000)], 1_000);
    fs::write(
        output.join("corrections.md"),
        format!("short. -> {}.\n", "expanded".repeat(20)),
    )
    .unwrap();

    let result = cue_command(temp.path())
        .args(["subtitles", "check", output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| {
                item["id"] == "CUE-SUBTITLE-LINE-CAPACITY"
                    || item["id"] == "CUE-SUBTITLE-READING-SPEED"
            })
    );
}

#[test]
fn corrected_diagnostics_retain_the_complete_canonical_phrase_span() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    write_transcript(
        &output,
        vec![word("short", 0, 400), word("phrase.", 410, 1_000)],
        1_000,
    );
    fs::write(
        output.join("corrections.md"),
        format!("short phrase. -> {}.\n", "expanded".repeat(20)),
    )
    .unwrap();

    let result = cue_command(temp.path())
        .args(["subtitles", "check", output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| {
                item["id"] == "CUE-SUBTITLE-LINE-CAPACITY"
                    && item["word_start"] == 0
                    && item["word_end"] == 2
            })
    );
}

#[test]
fn corrected_diagnostics_retain_source_spans_across_segments() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    let transcript = Transcript {
        schema_version: cue_core::TRANSCRIPT_SCHEMA_VERSION,
        language: "en".into(),
        duration_ms: 1_000,
        words: vec![word("cross", 0, 400), word("boundary.", 410, 1_000)],
        segments: vec![
            Segment {
                start_ms: 0,
                end_ms: 400,
                text: "cross".into(),
                word_start: 0,
                word_end: 1,
            },
            Segment {
                start_ms: 410,
                end_ms: 1_000,
                text: "boundary.".into(),
                word_start: 1,
                word_end: 2,
            },
        ],
    };
    write_transcript_value(&output, &transcript);
    fs::write(
        output.join("corrections.md"),
        format!("cross boundary. -> {}.\n", "expanded".repeat(20)),
    )
    .unwrap();

    let result = cue_command(temp.path())
        .args(["subtitles", "check", output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| {
                item["id"] == "CUE-SUBTITLE-LINE-CAPACITY"
                    && item["word_start"] == 0
                    && item["word_end"] == 2
            })
    );
}

#[test]
fn subtitle_check_reports_intrinsically_dropped_cues_in_json() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    write_transcript(&output, vec![word("instant.", 400, 400)], 400);

    let result = cue_command(temp.path())
        .args(["subtitles", "check", output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| {
                item["id"] == "CUE-SUBTITLE-TIMING-REPAIR"
                    && item["kind"] == "dropped"
                    && item["original_start_ms"] == 400
                    && item["original_end_ms"] == 400
            })
    );
}

#[test]
fn subtitle_check_reports_embedded_line_count_violations() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    write_transcript(&output, vec![word("one\ntwo\nthree", 0, 2_000)], 2_000);

    let result = cue_command(temp.path())
        .args(["subtitles", "check", output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["id"] == "CUE-SUBTITLE-LINE-CAPACITY" && item["line_count"] == 3 })
    );
}

#[test]
fn subtitle_check_reports_malformed_transcript_json() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir_all(&output).unwrap();
    fs::write(output.join("transcript.json"), b"not json").unwrap();

    let result = cue_command(temp.path())
        .args(["subtitles", "check", output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("could not parse"));
}

#[test]
fn subtitle_check_reports_missing_output_directories() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing.cue");

    let result = cue_command(temp.path())
        .args(["subtitles", "check", missing.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("no cue output directory found"));
}
