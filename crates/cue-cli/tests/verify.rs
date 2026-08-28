use std::path::Path;
use std::process::Command;

fn cue() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cue"))
}

fn digest(path: &Path) -> serde_json::Value {
    serde_json::json!({
        "algorithm": "blake3",
        "value": cue_cache::file_hash(path).expect("hash fixture"),
    })
}

struct Fixture {
    _temp: tempfile::TempDir,
    output: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("create temp directory");
    let source = temp.path().join("lesson.mp4");
    let output = temp.path().join("lesson.cue");
    std::fs::create_dir(&output).expect("create output directory");
    std::fs::write(&source, b"source media").expect("write source");
    std::fs::write(output.join("transcript.json"), b"{}\n").expect("write artifact");
    std::fs::write(output.join("transcript.txt"), b"Hello.\n").expect("write artifact");

    let receipt = serde_json::json!({
        "schema_version": 1,
        "cue_version": env!("CARGO_PKG_VERSION"),
        "mode": "transcript-only",
        "source": {
            "path": "../lesson.mp4",
            "digest": digest(&source),
        },
        "configuration": {
            "language": null,
            "transcription": {
                "provider": "faster-whisper",
                "model": "large-v3-turbo",
            },
            "normalization": {
                "provider": "s1",
                "ollama_url": "http://localhost:11434",
                "styling": "semi-formal",
                "structure": "prose",
                "context": "general",
            },
            "subtitles": {
                "formats": ["srt", "vtt"],
                "max_lines": 2,
                "max_chars_per_line": 42,
                "max_duration_ms": 6000,
            },
            "analysis": {
                "summary": true,
                "description": true,
                "chapters": true,
            },
            "llm": null,
        },
        "providers": [
            {"stage": "inspect", "provider": "ffprobe", "model": null, "endpoint": null},
            {"stage": "extract", "provider": "ffmpeg", "model": null, "endpoint": null},
            {"stage": "transcribe", "provider": "faster-whisper", "model": "large-v3-turbo", "endpoint": null}
        ],
        "stages": [
            {"stage": "inspect", "status": "executed", "detail": null},
            {"stage": "extract", "status": "cached", "detail": null},
            {"stage": "transcribe", "status": "cached", "detail": null},
            {"stage": "render", "status": "executed", "detail": null},
        ],
        "warnings": [],
        "remote_data_usage": {
            "normalized_text_sent_to_remote_in_current_run": null,
        },
        "corrections": [],
        "artifacts": [
            {
                "path": "transcript.json",
                "digest": digest(&output.join("transcript.json")),
            },
            {
                "path": "transcript.txt",
                "digest": digest(&output.join("transcript.txt")),
            },
        ],
    });
    std::fs::write(
        output.join("cue.run.json"),
        serde_json::to_vec_pretty(&receipt).expect("serialize receipt"),
    )
    .expect("write receipt");
    Fixture {
        _temp: temp,
        output,
    }
}

#[test]
fn verify_accepts_an_intact_run_receipt() {
    let fixture = fixture();

    let result = cue()
        .args([
            "verify",
            fixture.output.to_str().expect("utf-8 output path"),
        ])
        .output()
        .expect("run cue verify");

    assert!(result.status.success(), "{result:?}");
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("Verified 2 artifact"),
        "{result:?}"
    );
    assert!(result.stderr.is_empty(), "{result:?}");
}

#[test]
fn verify_json_reports_a_modified_artifact() {
    let fixture = fixture();
    std::fs::write(fixture.output.join("transcript.txt"), b"Changed.\n").expect("modify artifact");

    let result = cue()
        .args([
            "verify",
            fixture.output.to_str().expect("utf-8 output path"),
            "--json",
        ])
        .output()
        .expect("run cue verify --json");

    assert_eq!(result.status.code(), Some(1), "{result:?}");
    assert!(result.stderr.is_empty(), "{result:?}");
    let report: serde_json::Value =
        serde_json::from_slice(&result.stdout).expect("parse verification report");
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["valid"], false);
    assert_eq!(report["diagnostics"].as_array().unwrap().len(), 1);
    assert_eq!(
        report["diagnostics"][0]["id"],
        "CUE-VERIFY-ARTIFACT-MISMATCH"
    );
    assert_eq!(report["diagnostics"][0]["path"], "transcript.txt");
    assert!(report["diagnostics"][0]["expected_digest"].is_string());
    assert!(report["diagnostics"][0]["actual_digest"].is_string());
}

#[test]
fn verify_json_reports_a_modified_correction_manifest() {
    let fixture = fixture();
    let manifest = fixture.output.parent().unwrap().join("corrections.md");
    std::fs::write(&manifest, b"old -> New\n").expect("write correction manifest");
    let receipt_path = fixture.output.join("cue.run.json");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    receipt["corrections"] = serde_json::json!([{
        "path": "../corrections.md",
        "digest": digest(&manifest),
    }]);
    std::fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    std::fs::write(&manifest, b"old -> Changed\n").expect("modify correction manifest");

    let result = cue()
        .args([
            "verify",
            fixture.output.to_str().expect("utf-8 output path"),
            "--json",
        ])
        .output()
        .expect("run cue verify --json");

    assert_eq!(result.status.code(), Some(1), "{result:?}");
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        report["diagnostics"][0]["id"],
        "CUE-VERIFY-CORRECTION-MISMATCH"
    );
    assert_eq!(report["diagnostics"][0]["path"], "../corrections.md");
}

#[test]
fn verify_rejects_artifact_paths_outside_the_output_directory() {
    let fixture = fixture();
    let receipt_path = fixture.output.join("cue.run.json");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    receipt["artifacts"][0]["path"] = serde_json::json!("../lesson.mp4");
    std::fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();

    let result = cue()
        .args([
            "verify",
            fixture.output.to_str().expect("utf-8 output path"),
        ])
        .output()
        .expect("run cue verify");

    assert_eq!(result.status.code(), Some(1), "{result:?}");
    assert!(result.stdout.is_empty(), "{result:?}");
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("unsafe artifact path"),
        "{result:?}"
    );
}

#[test]
fn verify_json_reports_missing_and_malformed_receipts() {
    let fixture = fixture();
    let receipt = fixture.output.join("cue.run.json");
    std::fs::remove_file(&receipt).unwrap();

    let missing = cue()
        .args(["verify", fixture.output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1), "{missing:?}");
    assert!(missing.stderr.is_empty(), "{missing:?}");
    let report: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(report["diagnostics"][0]["id"], "CUE-VERIFY-RECEIPT-MISSING");

    std::fs::write(&receipt, b"not json\n").unwrap();
    let malformed = cue()
        .args(["verify", fixture.output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(malformed.status.code(), Some(1), "{malformed:?}");
    assert!(malformed.stderr.is_empty(), "{malformed:?}");
    let report: serde_json::Value = serde_json::from_slice(&malformed.stdout).unwrap();
    assert_eq!(
        report["diagnostics"][0]["id"],
        "CUE-VERIFY-RECEIPT-MALFORMED"
    );
}

#[cfg(unix)]
#[test]
fn verify_rejects_a_symlinked_run_receipt() {
    use std::os::unix::fs::symlink;

    let fixture = fixture();
    let receipt = fixture.output.join("cue.run.json");
    let external = fixture.output.parent().unwrap().join("external-run.json");
    std::fs::rename(&receipt, &external).unwrap();
    symlink(&external, &receipt).unwrap();

    let result = cue()
        .args(["verify", fixture.output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1), "{result:?}");
    assert!(result.stderr.is_empty(), "{result:?}");
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["diagnostics"][0]["id"], "CUE-VERIFY-RECEIPT-UNSAFE");
}

#[test]
fn verify_json_reports_unsupported_receipt_schemas() {
    let fixture = fixture();
    let receipt_path = fixture.output.join("cue.run.json");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    receipt["schema_version"] = serde_json::json!(2);
    std::fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();

    let result = cue()
        .args(["verify", fixture.output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1), "{result:?}");
    assert!(result.stderr.is_empty(), "{result:?}");
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        report["diagnostics"][0]["id"],
        "CUE-VERIFY-SCHEMA-UNSUPPORTED"
    );
}

#[test]
fn verify_json_reports_source_drift_and_missing_artifacts() {
    let fixture = fixture();
    let source = fixture.output.parent().unwrap().join("lesson.mp4");
    std::fs::write(&source, b"changed source").unwrap();
    std::fs::remove_file(fixture.output.join("transcript.json")).unwrap();

    let result = cue()
        .args(["verify", fixture.output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1), "{result:?}");
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let ids: Vec<_> = report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"CUE-VERIFY-SOURCE-MISMATCH"));
    assert!(ids.contains(&"CUE-VERIFY-ARTIFACT-MISSING"));
}

#[cfg(unix)]
#[test]
fn verify_rejects_symlinked_artifacts() {
    use std::os::unix::fs::symlink;

    let fixture = fixture();
    let artifact = fixture.output.join("transcript.txt");
    let external = fixture.output.parent().unwrap().join("external.txt");
    std::fs::write(&external, b"external\n").unwrap();
    std::fs::remove_file(&artifact).unwrap();
    symlink(&external, &artifact).unwrap();

    let result = cue()
        .args(["verify", fixture.output.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1), "{result:?}");
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["diagnostics"][0]["id"], "CUE-VERIFY-ARTIFACT-UNSAFE");
}
