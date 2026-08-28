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
    fs::write(output.join("cue.run.json"), "STALE RUN RECEIPT\n").unwrap();
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
    assert_eq!(receipt["schema_version"], 2);
    assert_eq!(
        receipt["manifests"][0]["hash"],
        cue_cache::bytes_hash(manifest_bytes)
    );
    assert_eq!(
        receipt["source_hashes"]["transcript"],
        cue_cache::bytes_hash(&transcript_bytes)
    );
    assert!(receipt["source_hashes"]["normalized"].is_null());
    assert_eq!(receipt["manifests"][0]["source"], "explicit");
    assert_eq!(receipt["rules"][0]["source_manifest"], 0);
    assert_eq!(receipt["rules"][0]["find"], "open telemetry");
    assert_eq!(receipt["rules"][0]["replace"], "OpenTelemetry");
    assert_eq!(
        receipt["rules"][0]["applications"][0]["artifact"],
        "transcript.txt"
    );
    assert_eq!(receipt["rules"][0]["applications"][0]["replacements"], 1);
    assert!(!output.join("cue.run.json").exists());
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
    assert!(vtt.starts_with("WEBVTT\n\n"), "{vtt}");
    assert!(!srt.contains("SENTINEL"), "{srt}");
    assert!(!vtt.contains("SENTINEL"), "{vtt}");
}

#[test]
fn subtitle_corrections_can_span_cue_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    let transcript_path = output.join("transcript.json");
    let transcript = fs::read_to_string(&transcript_path)
        .unwrap()
        .replace("\"duration_ms\": 1000", "\"duration_ms\": 2000")
        .replace(
            "\"start_ms\":310,\"end_ms\":1000",
            "\"start_ms\":1000,\"end_ms\":2000",
        );
    fs::write(transcript_path, transcript).unwrap();
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
    assert!(srt.contains("OpenTelemetry"), "{srt}");
    assert!(!srt.contains("open\ntelemetry"), "{srt}");
    assert!(!output.join("subtitles.vtt").exists());
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

    let result = cue_command(temp.path())
        .args([
            "correct",
            output.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("no replacements; derived artifacts rebuilt from canonical sources"),
        "{stdout}"
    );
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

#[test]
fn output_manifest_takes_precedence_over_parent_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(
        temp.path().join("corrections.md"),
        "open telemetry -> Parent\n",
    )
    .unwrap();
    fs::write(output.join("corrections.md"), "open telemetry -> Output\n").unwrap();

    let status = cue_command(temp.path())
        .args(["correct", output.to_str().unwrap()])
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(output.join("transcript.txt")).unwrap(),
        "Output.\n"
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("corrections.applied.json")).unwrap())
            .unwrap();
    assert_eq!(
        receipt["manifests"].as_array().unwrap().last().unwrap()["source"],
        "output-directory"
    );
}

#[test]
fn parent_manifest_is_used_when_output_manifest_is_absent() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(
        temp.path().join("corrections.md"),
        "open telemetry -> Parent\n",
    )
    .unwrap();

    let status = cue_command(temp.path())
        .args(["correct", output.to_str().unwrap()])
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(output.join("transcript.txt")).unwrap(),
        "Parent.\n"
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("corrections.applied.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["manifests"][0]["source"], "parent-directory");
}

#[test]
fn discovered_lexicons_compose_from_project_to_source_with_nearest_rule_winning() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let course = project.join("course");
    let output = course.join("lesson.cue");
    fs::create_dir_all(&output).unwrap();
    write_transcript(&output);
    fs::write(
        project.join("corrections.md"),
        "open -> OPEN\ntelemetry -> ProjectTelemetry\n",
    )
    .unwrap();
    fs::write(
        course.join("corrections.md"),
        "telemetry -> CourseTelemetry\n",
    )
    .unwrap();

    let result = cue_command(temp.path())
        .current_dir(&project)
        .args(["correct", output.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read_to_string(output.join("transcript.txt")).unwrap(),
        "OPEN CourseTelemetry.\n"
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("corrections.applied.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["schema_version"], 2);
    assert_eq!(receipt["manifests"].as_array().unwrap().len(), 2);
    assert_eq!(receipt["manifests"][0]["path"], "../../corrections.md");
    assert_eq!(receipt["manifests"][1]["path"], "../corrections.md");
    assert_eq!(receipt["rules"][0]["source_manifest"], 0);
    assert_eq!(receipt["rules"][1]["source_manifest"], 1);
}

#[test]
fn empty_broad_discovered_lexicon_does_not_hide_valid_nearer_lexicon() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let course = project.join("course");
    let output = course.join("lesson.cue");
    fs::create_dir_all(&output).unwrap();
    write_transcript(&output);
    fs::write(
        project.join("corrections.md"),
        "# no project-wide corrections yet\n",
    )
    .unwrap();
    fs::write(
        course.join("corrections.md"),
        "open telemetry -> OpenTelemetry\n",
    )
    .unwrap();

    let result = cue_command(temp.path())
        .current_dir(&project)
        .args(["correct", output.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read_to_string(output.join("transcript.txt")).unwrap(),
        "OpenTelemetry.\n"
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("corrections.applied.json")).unwrap())
            .unwrap();
    let manifests = receipt["manifests"].as_array().unwrap();
    assert_eq!(manifests.len(), 1);
    assert_eq!(manifests[0]["path"], "../corrections.md");
}

#[test]
fn empty_discovered_lexicon_is_treated_as_no_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(
        temp.path().join("corrections.md"),
        "# no approved corrections yet\n",
    )
    .unwrap();

    let result = cue_command(temp.path())
        .args(["correct", output.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("no corrections manifest found"), "{stderr}");
    assert!(!stderr.contains("manifest has no rules"), "{stderr}");
    assert!(!output.join("corrections.applied.json").exists());
}

#[test]
fn nearest_override_runs_at_its_near_scope_position() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let course = project.join("course");
    let output = course.join("lesson.cue");
    fs::create_dir_all(&output).unwrap();
    write_transcript(&output);
    fs::write(
        project.join("corrections.md"),
        "open -> Broad\ntelemetry -> open\n",
    )
    .unwrap();
    fs::write(course.join("corrections.md"), "open -> Near\n").unwrap();

    let result = cue_command(temp.path())
        .current_dir(&project)
        .args(["correct", output.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(result.status.success());
    assert_eq!(
        fs::read_to_string(output.join("transcript.txt")).unwrap(),
        "Near Near.\n"
    );
}

#[test]
fn output_at_working_directory_does_not_discover_parent_lexicon() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(
        temp.path().join("corrections.md"),
        "open telemetry -> OutsideBoundary\n",
    )
    .unwrap();

    let result = cue_command(temp.path())
        .current_dir(&output)
        .args(["correct", "."])
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("no corrections manifest found"));
}

#[test]
fn lexicon_promote_copies_an_applied_rule_to_an_explicit_scope() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let source = project.join("course");
    let output = source.join("lesson.cue");
    fs::create_dir_all(&output).unwrap();
    write_transcript(&output);
    fs::write(
        source.join("corrections.md"),
        "open telemetry -> OpenTelemetry\n",
    )
    .unwrap();
    let corrected = cue_command(temp.path())
        .current_dir(&project)
        .args(["correct", output.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(corrected.status.success());

    let promoted = cue_command(temp.path())
        .current_dir(&project)
        .args([
            "lexicon",
            "promote",
            output.to_str().unwrap(),
            "--rule",
            "open telemetry",
            "--to",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        promoted.status.success(),
        "{}",
        String::from_utf8_lossy(&promoted.stderr)
    );
    assert_eq!(
        fs::read_to_string(project.join("corrections.md")).unwrap(),
        "open telemetry -> OpenTelemetry\n"
    );

    let attestation = cue_command(temp.path())
        .current_dir(&project)
        .args([
            "lexicon",
            "promote",
            output.to_str().unwrap(),
            "--rule",
            "open telemetry",
            "--to",
            project.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(attestation.status.success());
    let attestation: serde_json::Value = serde_json::from_slice(&attestation.stdout).unwrap();
    assert_eq!(attestation["schema_version"], 1);
    assert_eq!(attestation["status"], "already-present");
    assert_eq!(
        attestation["source_receipt_hash"],
        cue_cache::bytes_hash(&fs::read(output.join("corrections.applied.json")).unwrap())
    );
    assert_eq!(
        attestation["target_lexicon_hash"],
        cue_cache::bytes_hash(&fs::read(project.join("corrections.md")).unwrap())
    );
}

#[test]
fn lexicon_promote_rejects_a_rule_that_never_applied() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let output = project.join("lesson.cue");
    fs::create_dir_all(&output).unwrap();
    write_transcript(&output);
    fs::write(
        output.join("corrections.md"),
        "missing phrase -> Better Phrase\n",
    )
    .unwrap();
    let corrected = cue_command(temp.path())
        .current_dir(&project)
        .args(["correct", output.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(corrected.status.success());

    let promoted = cue_command(temp.path())
        .current_dir(&project)
        .args([
            "lexicon",
            "promote",
            output.to_str().unwrap(),
            "--rule",
            "missing phrase",
            "--to",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!promoted.status.success());
    assert!(
        String::from_utf8_lossy(&promoted.stderr).contains("did not match any rendered artifact")
    );
    assert!(!project.join("corrections.md").exists());
}

#[test]
fn lexicon_promote_rejects_stale_receipt_provenance() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let output = project.join("lesson.cue");
    fs::create_dir_all(&output).unwrap();
    write_transcript(&output);
    fs::write(
        output.join("corrections.md"),
        "open telemetry -> OpenTelemetry\n",
    )
    .unwrap();
    assert!(
        cue_command(temp.path())
            .current_dir(&project)
            .args(["correct", output.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    fs::write(output.join("transcript.json"), "{}\n").unwrap();

    let promoted = cue_command(temp.path())
        .current_dir(&project)
        .args([
            "lexicon",
            "promote",
            output.to_str().unwrap(),
            "--rule",
            "open telemetry",
            "--to",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!promoted.status.success());
    assert!(String::from_utf8_lossy(&promoted.stderr).contains("no longer matches"));
    assert!(!project.join("corrections.md").exists());

    write_transcript(&output);
    assert!(
        cue_command(temp.path())
            .current_dir(&project)
            .args(["correct", output.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        output.join("corrections.md"),
        "open telemetry -> ChangedAfterRender\n",
    )
    .unwrap();
    let promoted = cue_command(temp.path())
        .current_dir(&project)
        .args([
            "lexicon",
            "promote",
            output.to_str().unwrap(),
            "--rule",
            "open telemetry",
            "--to",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!promoted.status.success());
    assert!(String::from_utf8_lossy(&promoted.stderr).contains("no longer matches"));
    assert!(!project.join("corrections.md").exists());
}

#[test]
fn lexicon_promote_rejects_a_receipt_owned_mapping() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let output = project.join("lesson.cue");
    fs::create_dir_all(&output).unwrap();
    write_transcript(&output);
    fs::write(
        output.join("corrections.md"),
        "open telemetry -> OpenTelemetry\n",
    )
    .unwrap();
    assert!(
        cue_command(temp.path())
            .current_dir(&project)
            .args(["correct", output.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    let receipt_path = output.join("corrections.applied.json");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    receipt["rules"][0]["replace"] = "Fabricated".into();
    fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();

    let promoted = cue_command(temp.path())
        .current_dir(&project)
        .args([
            "lexicon",
            "promote",
            output.to_str().unwrap(),
            "--rule",
            "open telemetry",
            "--to",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!promoted.status.success());
    assert!(String::from_utf8_lossy(&promoted.stderr).contains("did not match"));
    assert!(!project.join("corrections.md").exists());
}

#[test]
fn lexicon_promote_is_idempotent_and_does_not_overwrite_conflicts() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let source = project.join("source");
    let output = source.join("lesson.cue");
    fs::create_dir_all(&output).unwrap();
    write_transcript(&output);
    fs::write(
        source.join("corrections.md"),
        "open telemetry -> OpenTelemetry\n",
    )
    .unwrap();
    assert!(
        cue_command(temp.path())
            .current_dir(&project)
            .args(["correct", output.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );

    let promote = || {
        cue_command(temp.path())
            .current_dir(&project)
            .args([
                "lexicon",
                "promote",
                output.to_str().unwrap(),
                "--rule",
                "open telemetry",
                "--to",
                project.to_str().unwrap(),
            ])
            .output()
            .unwrap()
    };
    assert!(promote().status.success());
    assert!(promote().status.success());
    assert_eq!(
        fs::read_to_string(project.join("corrections.md")).unwrap(),
        "open telemetry -> OpenTelemetry\n"
    );

    fs::write(
        project.join("corrections.md"),
        "open telemetry -> DifferentSpelling\n",
    )
    .unwrap();
    let conflict = promote();
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("already maps"));
    assert_eq!(
        fs::read_to_string(project.join("corrections.md")).unwrap(),
        "open telemetry -> DifferentSpelling\n"
    );
}

#[test]
fn explicit_manifest_takes_precedence_over_discovered_manifests() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(
        temp.path().join("corrections.md"),
        "open telemetry -> Parent\n",
    )
    .unwrap();
    fs::write(output.join("corrections.md"), "open telemetry -> Output\n").unwrap();
    let explicit = temp.path().join("explicit.md");
    fs::write(&explicit, "open telemetry -> Explicit\n").unwrap();

    let status = cue_command(temp.path())
        .args([
            "correct",
            output.to_str().unwrap(),
            "--corrections",
            explicit.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(output.join("transcript.txt")).unwrap(),
        "Explicit.\n"
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("corrections.applied.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["manifests"][0]["source"], "explicit");
}

#[test]
fn correct_requires_a_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(output.join("transcript.txt"), "UNCHANGED\n").unwrap();

    let result = cue_command(temp.path())
        .args(["correct", output.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert_eq!(
        fs::read_to_string(output.join("transcript.txt")).unwrap(),
        "UNCHANGED\n"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("no corrections manifest found"), "{stderr}");
}

#[test]
fn correct_rejects_an_empty_manifest_without_changing_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("lesson.cue");
    fs::create_dir(&output).unwrap();
    write_transcript(&output);
    fs::write(output.join("transcript.txt"), "UNCHANGED\n").unwrap();
    let manifest = temp.path().join("empty.md");
    fs::write(&manifest, "# no approved corrections yet\n\n").unwrap();

    let result = cue_command(temp.path())
        .args([
            "correct",
            output.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert_eq!(
        fs::read_to_string(output.join("transcript.txt")).unwrap(),
        "UNCHANGED\n"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("manifest has no rules"), "{stderr}");
    assert!(stderr.contains(&manifest.display().to_string()), "{stderr}");
}

fn write_dummy_batch(root: &std::path::Path) -> [std::path::PathBuf; 2] {
    let first = root.join("first.mp4");
    let second = root.join("second.mp4");
    fs::write(&first, b"not media").unwrap();
    fs::write(&second, b"not media").unwrap();
    for name in ["first.cue", "second.cue"] {
        let output = root.join(name);
        fs::create_dir(&output).unwrap();
        fs::write(output.join("sentinel"), b"UNCHANGED\n").unwrap();
    }
    [first, second]
}

fn assert_batch_outputs_unchanged(root: &std::path::Path) {
    for name in ["first.cue", "second.cue"] {
        assert_eq!(
            fs::read(root.join(name).join("sentinel")).unwrap(),
            b"UNCHANGED\n"
        );
    }
}

#[test]
fn missing_explicit_manifest_fails_batch_before_media_inspection() {
    let temp = tempfile::tempdir().unwrap();
    let [first, second] = write_dummy_batch(temp.path());
    let missing = temp.path().join("missing.md");

    let result = cue_command(temp.path())
        .args([
            first.to_str().unwrap(),
            second.to_str().unwrap(),
            "--corrections",
            missing.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("manifest"), "{stderr}");
    assert!(stderr.contains("does not exist"), "{stderr}");
    assert!(!stderr.contains("ffprobe"), "{stderr}");
    assert_batch_outputs_unchanged(temp.path());
}

#[test]
fn malformed_explicit_manifest_fails_batch_before_media_inspection() {
    let temp = tempfile::tempdir().unwrap();
    let [first, second] = write_dummy_batch(temp.path());
    let manifest = temp.path().join("malformed.md");
    fs::write(&manifest, "this is not a correction rule\n").unwrap();

    let result = cue_command(temp.path())
        .args([
            first.to_str().unwrap(),
            second.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("manifest line 1"), "{stderr}");
    assert!(!stderr.contains("ffprobe"), "{stderr}");
    assert_batch_outputs_unchanged(temp.path());
}

#[test]
fn empty_explicit_manifest_fails_batch_before_media_inspection() {
    let temp = tempfile::tempdir().unwrap();
    let [first, second] = write_dummy_batch(temp.path());
    let manifest = temp.path().join("empty.md");
    fs::write(&manifest, "# no approved corrections yet\n").unwrap();

    let result = cue_command(temp.path())
        .args([
            first.to_str().unwrap(),
            second.to_str().unwrap(),
            "--corrections",
            manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("manifest has no rules"), "{stderr}");
    assert!(stderr.contains(&manifest.display().to_string()), "{stderr}");
    assert!(!stderr.contains("ffprobe"), "{stderr}");
    assert_batch_outputs_unchanged(temp.path());
}

#[test]
fn invalid_discovered_manifest_fails_the_whole_batch_before_media_inspection() {
    let temp = tempfile::tempdir().unwrap();
    let [first, second] = write_dummy_batch(temp.path());
    fs::write(
        temp.path().join("first.cue/corrections.md"),
        "open telemetry -> OpenTelemetry\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("second.cue/corrections.md"),
        "not a correction rule\n",
    )
    .unwrap();

    let result = cue_command(temp.path())
        .args([first.to_str().unwrap(), second.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("manifest line 1"), "{stderr}");
    assert!(!stderr.contains("ffprobe"), "{stderr}");
    assert_batch_outputs_unchanged(temp.path());
}
