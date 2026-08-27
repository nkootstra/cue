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
    {"text":"open","start_ms":0,"end_ms":300,"confidence":0.9,"speaker":null},
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
fn correct_rebuilds_transcript_text_from_canonical_json() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(output.join("transcript.txt"), "MANUAL SENTINEL\n").unwrap();
    let manifest = temp.path().join("corrections.md");
    fs::write(&manifest, "open telemetry -> OpenTelemetry\n").unwrap();

    let status = cue_command(temp.path())
        .args([
            "correct",
            output.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(output.join("transcript.txt")).unwrap(),
        "OpenTelemetry.\n"
    );
}

#[test]
fn correct_writes_a_versioned_receipt_for_the_canonical_sources() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(output.join("transcript.txt"), "old render\n").unwrap();
    let manifest = temp.path().join("corrections.md");
    let manifest_bytes = b"open telemetry -> OpenTelemetry\n";
    fs::write(&manifest, manifest_bytes).unwrap();
    let transcript_bytes = fs::read(output.join("transcript.json")).unwrap();

    let status = cue_command(temp.path())
        .args([
            "correct",
            output.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(status.success());
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("corrections.applied.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["schema_version"], 1);
    assert_eq!(
        receipt["manifest_hash"],
        cue_cache::bytes_hash(manifest_bytes)
    );
    assert_eq!(
        receipt["source_hashes"]["transcript"],
        cue_cache::bytes_hash(&transcript_bytes)
    );
    assert!(receipt["source_hashes"]["normalized"].is_null());
    assert_eq!(receipt["manifest_source"], "explicit");
    assert_eq!(receipt["rules"][0]["find"], "open telemetry");
    assert_eq!(receipt["rules"][0]["replace"], "OpenTelemetry");
    assert_eq!(
        receipt["rules"][0]["applications"][0]["artifact"],
        "transcript.txt"
    );
    assert_eq!(receipt["rules"][0]["applications"][0]["replacements"], 1);
}

#[test]
fn correct_rebuilds_clean_text_from_normalized_json() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(output.join("transcript.txt"), "old render\n").unwrap();
    fs::write(
        output.join("normalized.json"),
        r#"{
  "schema_version": 1,
  "chunks": [
    {"start_ms":0,"end_ms":1000,"text":"Using open telemetry."}
  ]
}"#,
    )
    .unwrap();
    fs::write(output.join("transcript.clean.txt"), "MANUAL SENTINEL\n").unwrap();
    let manifest = temp.path().join("corrections.md");
    fs::write(&manifest, "open telemetry -> OpenTelemetry\n").unwrap();

    let status = cue_command(temp.path())
        .args([
            "correct",
            output.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(output.join("transcript.clean.txt")).unwrap(),
        "Using OpenTelemetry.\n"
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("corrections.applied.json")).unwrap())
            .unwrap();
    assert_eq!(
        receipt["source_hashes"]["normalized"],
        cue_cache::bytes_hash(&fs::read(output.join("normalized.json")).unwrap())
    );
}

#[test]
fn correct_regenerates_subtitles_from_the_canonical_transcript() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(output.join("transcript.txt"), "old render\n").unwrap();
    fs::write(output.join("subtitles.srt"), "MANUAL SRT SENTINEL\n").unwrap();
    fs::write(output.join("subtitles.vtt"), "MANUAL VTT SENTINEL\n").unwrap();
    let manifest = temp.path().join("corrections.md");
    fs::write(&manifest, "open telemetry -> OpenTelemetry\n").unwrap();

    let status = cue_command(temp.path())
        .args([
            "correct",
            output.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(status.success());
    let srt = fs::read_to_string(output.join("subtitles.srt")).unwrap();
    let vtt = fs::read_to_string(output.join("subtitles.vtt")).unwrap();
    assert!(srt.contains("OpenTelemetry."), "{srt}");
    assert!(vtt.contains("OpenTelemetry."), "{vtt}");
    assert!(!srt.contains("SENTINEL"), "{srt}");
    assert!(!vtt.contains("SENTINEL"), "{vtt}");
}

#[test]
fn valid_zero_match_manifest_still_rebuilds_and_records_the_render() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(output.join("transcript.txt"), "MANUAL SENTINEL\n").unwrap();
    let manifest = temp.path().join("corrections.md");
    fs::write(&manifest, "missing phrase -> Better Phrase\n").unwrap();

    let status = cue_command(temp.path())
        .args([
            "correct",
            output.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(output.join("transcript.txt")).unwrap(),
        "open telemetry.\n"
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("corrections.applied.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["rules"][0]["applications"][0]["replacements"], 0);
}

#[test]
fn dry_run_does_not_write_or_remove_any_output() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(output.join("transcript.txt"), "MANUAL SENTINEL\n").unwrap();
    fs::write(output.join("transcript.clean.txt"), "STALE CLEAN\n").unwrap();
    fs::write(output.join("corrections.applied.json"), "OLD RECEIPT\n").unwrap();
    let manifest = temp.path().join("corrections.md");
    fs::write(&manifest, "open telemetry -> OpenTelemetry\n").unwrap();

    let before = [
        "transcript.json",
        "transcript.txt",
        "transcript.clean.txt",
        "corrections.applied.json",
    ]
    .map(|name| (name, fs::read(output.join(name)).unwrap()));

    let status = cue_command(temp.path())
        .args([
            "correct",
            output.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
            "--dry-run",
        ])
        .status()
        .unwrap();

    assert!(status.success());
    for (name, bytes) in before {
        assert_eq!(fs::read(output.join(name)).unwrap(), bytes, "{name}");
    }
    assert!(!output.join("subtitles.srt").exists());
    assert!(!output.join("subtitles.vtt").exists());
}

#[test]
fn correct_leaves_canonical_and_analysis_artifacts_byte_identical() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(output.join("transcript.txt"), "old render\n").unwrap();
    fs::write(
        output.join("normalized.json"),
        r#"{"schema_version":1,"chunks":[{"start_ms":0,"end_ms":1000,"text":"open telemetry"}]}"#,
    )
    .unwrap();
    fs::write(
        output.join("analysis.json"),
        b"{\n  \"term\": \"open telemetry\"\n}\n",
    )
    .unwrap();
    fs::write(output.join("summary.md"), b"# open telemetry\n").unwrap();
    fs::write(
        output.join("description.md"),
        b"open telemetry description\n",
    )
    .unwrap();
    let manifest = temp.path().join("corrections.md");
    fs::write(&manifest, "open telemetry -> OpenTelemetry\n").unwrap();

    let untouched = [
        "transcript.json",
        "normalized.json",
        "analysis.json",
        "summary.md",
        "description.md",
    ]
    .map(|name| (name, fs::read(output.join(name)).unwrap()));

    let status = cue_command(temp.path())
        .args([
            "correct",
            output.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(status.success());
    for (name, bytes) in untouched {
        assert_eq!(fs::read(output.join(name)).unwrap(), bytes, "{name}");
    }
}

#[test]
fn changing_the_manifest_replaces_the_prior_render_from_canonical_input() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(output.join("transcript.txt"), "old render\n").unwrap();
    let manifest = temp.path().join("corrections.md");
    fs::write(&manifest, "open telemetry -> First Name\n").unwrap();

    let first = cue_command(temp.path())
        .args([
            "correct",
            output.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(first.success());
    assert_eq!(
        fs::read_to_string(output.join("transcript.txt")).unwrap(),
        "First Name.\n"
    );

    fs::write(&manifest, "open telemetry -> Second Name\n").unwrap();
    let second = cue_command(temp.path())
        .args([
            "correct",
            output.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(second.success());
    assert_eq!(
        fs::read_to_string(output.join("transcript.txt")).unwrap(),
        "Second Name.\n"
    );
}

#[test]
fn correct_removes_stale_clean_text_without_normalized_json() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(output.join("transcript.txt"), "old render\n").unwrap();
    fs::write(output.join("transcript.clean.txt"), "STALE CLEAN\n").unwrap();
    let manifest = temp.path().join("corrections.md");
    fs::write(&manifest, "open telemetry -> OpenTelemetry\n").unwrap();

    let status = cue_command(temp.path())
        .args([
            "correct",
            output.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(status.success());
    assert!(!output.join("transcript.clean.txt").exists());
}
