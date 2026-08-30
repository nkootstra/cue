use std::fs;
use std::process::Command;

fn cue_command(config_dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cue"));
    command.env("XDG_CONFIG_HOME", config_dir);
    command
}

fn write_transcript(output: &std::path::Path) {
    fs::write(
        output.join("transcript.json"),
        r#"{
  "schema_version": 1,
  "language": "en",
  "duration_ms": 1000,
  "words": [
    {"text":"open","start_ms":0,"end_ms":300,"confidence":0.4,"speaker":null},
    {"text":"telemetry.","start_ms":310,"end_ms":1000,"confidence":0.9,"speaker":null}
  ],
  "segments": [
    {"start_ms":0,"end_ms":1000,"text":"open telemetry.","word_start":0,"word_end":2}
  ]
}"#,
    )
    .unwrap();
}

#[test]
fn review_json_surfaces_low_confidence_unmatched_rules_and_scope_conflicts() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let course = project.join("course");
    let output = course.join("lesson.cue");
    fs::create_dir_all(&output).unwrap();
    write_transcript(&output);
    fs::write(
        project.join("corrections.md"),
        "telemetry -> ProjectTelemetry\nunused term -> UsedTerm\n",
    )
    .unwrap();
    fs::write(
        course.join("corrections.md"),
        "telemetry -> CourseTelemetry\n",
    )
    .unwrap();

    let result = cue_command(temp.path())
        .current_dir(&project)
        .args([
            "review",
            output.to_str().unwrap(),
            "--confidence-below",
            "0.5",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["schema_version"], 2);
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert!(diagnostics.iter().any(|item| {
        item["id"] == "CUE-REVIEW-LOW-CONFIDENCE"
            && item["word"] == "open"
            && item["confidence"] == 0.4
    }));
    assert!(diagnostics.iter().any(|item| {
        item["id"] == "CUE-REVIEW-UNMATCHED-RULE" && item["find"] == "unused term"
    }));
    assert!(diagnostics.iter().any(|item| {
        item["id"] == "CUE-REVIEW-SCOPE-CONFLICT"
            && item["find"] == "telemetry"
            && item["winner"] == "CourseTelemetry"
            && item["shadowed"] == "ProjectTelemetry"
    }));
}

#[test]
fn review_finds_code_term_typo_and_accepts_it_into_output_scope() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("lesson.mp4");
    fs::write(&source, b"media").unwrap();
    let output = root.join(".cue/lesson");
    fs::create_dir_all(&output).unwrap();
    fs::write(
        output.join("cue.workspace.json"),
        r#"{"schema_version":1,"source":"../../lesson.mp4"}"#,
    )
    .unwrap();
    fs::write(root.join("Cargo.toml"), "manifest = cargo.toml\n").unwrap();
    fs::write(root.join("foo.toml"), "manifest = foo.toml\n").unwrap();
    fs::write(
        output.join("transcript.json"),
        r#"{
  "schema_version": 1, "language": "en", "duration_ms": 100,
  "words": [
    {"text":"cargo","start_ms":0,"end_ms":40,"confidence":0.99,"speaker":null},
    {"text":".tomo,","start_ms":50,"end_ms":100,"confidence":0.7633,"speaker":null}
  ],
  "segments": [{"start_ms":0,"end_ms":100,"text":"cargo .tomo,","word_start":0,"word_end":2}]
}"#,
    )
    .unwrap();

    let review = cue_command(root)
        .args(["review", output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        review.status.success(),
        "{}",
        String::from_utf8_lossy(&review.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&review.stdout).unwrap();
    let candidate = report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "CUE-REVIEW-TERM-MISMATCH")
        .unwrap();
    assert_eq!(candidate["proposed"], "cargo.toml");
    assert_eq!(candidate["candidate_id"], "term-0");
    assert_eq!(candidate["observed"], "cargo .tomo,");

    let accepted = cue_command(root)
        .args(["review", output.to_str().unwrap(), "--accept", "term-0"])
        .output()
        .unwrap();
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert_eq!(
        fs::read_to_string(output.join("corrections.md")).unwrap(),
        "cargo .tomo -> cargo.toml\n"
    );

    let corrected = cue_command(root)
        .args(["correct", output.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        corrected.status.success(),
        "{}",
        String::from_utf8_lossy(&corrected.stderr)
    );
    assert_eq!(
        fs::read_to_string(output.join("transcript.txt")).unwrap(),
        "cargo.toml,\n"
    );
}

#[test]
fn review_surfaces_possible_fallback_timing_and_ambiguous_speakers() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    fs::write(
        output.join("transcript.json"),
        r#"{
  "schema_version": 1,
  "language": "en",
  "duration_ms": 1000,
  "words": [
    {"text":"hello","start_ms":0,"end_ms":1000,"confidence":0.9,"speaker":"spk_0"},
    {"text":"world","start_ms":0,"end_ms":1000,"confidence":0.8,"speaker":null}
  ],
  "segments": [
    {"start_ms":0,"end_ms":1000,"text":"hello world","word_start":0,"word_end":2}
  ]
}"#,
    )
    .unwrap();

    let result = cue_command(temp.path())
        .args(["review", output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert!(result.status.success());
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert_eq!(
        diagnostics
            .iter()
            .filter(|item| item["id"] == "CUE-REVIEW-POSSIBLE-FALLBACK-TIMING")
            .count(),
        2
    );
    assert!(
        diagnostics
            .iter()
            .any(|item| item["id"] == "CUE-REVIEW-AMBIGUOUS-SPEAKER-TURN")
    );
}

#[test]
fn review_does_not_treat_missing_confidence_as_fallback_timing() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    fs::write(
        output.join("transcript.json"),
        r#"{
  "schema_version": 1,
  "language": "en",
  "duration_ms": 1000,
  "words": [
    {"text":"hello","start_ms":0,"end_ms":400,"confidence":null,"speaker":null},
    {"text":"world","start_ms":410,"end_ms":1000,"confidence":null,"speaker":null}
  ],
  "segments": [
    {"start_ms":0,"end_ms":1000,"text":"hello world","word_start":0,"word_end":2}
  ]
}"#,
    )
    .unwrap();

    let result = cue_command(temp.path())
        .args(["review", output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert!(result.status.success());
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        !report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["id"] == "CUE-REVIEW-POSSIBLE-FALLBACK-TIMING" })
    );
}

#[test]
fn review_rejects_confidence_thresholds_outside_probability_range() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);

    let result = cue_command(temp.path())
        .args([
            "review",
            output.to_str().unwrap(),
            "--confidence-below",
            "1.1",
        ])
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("between 0.0 and 1.0"));
}
