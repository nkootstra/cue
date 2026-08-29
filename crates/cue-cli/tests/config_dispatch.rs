use std::path::Path;
use std::process::Command;

fn invalid_config_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("create test config directory");
    std::fs::write(dir.path().join("cue.toml"), "this is not valid TOML = [")
        .expect("write invalid test config");
    dir
}

fn cue(config_dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cue"));
    command.env("CUE_CONFIG_DIR", config_dir);
    command
}

fn empty_media(directory: &Path, name: &str) -> std::path::PathBuf {
    let path = directory.join(name);
    std::fs::write(&path, []).expect("create empty test media");
    path
}

fn write_llm_config(config_dir: &Path, api_key_env: &str) {
    std::fs::write(
        config_dir.join("cue.toml"),
        format!(
            r#"[llm]
base_url = "http://127.0.0.1:9/v1"
model = "test-model"
api_key_env = {api_key_env:?}
"#
        ),
    )
    .expect("write test LLM configuration");
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn bare_models_help_does_not_load_user_configuration() {
    let config_dir = invalid_config_dir();
    let output = cue(config_dir.path())
        .arg("models")
        .output()
        .expect("run cue models");

    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("cue models list"));
}

#[test]
fn config_path_does_not_load_user_configuration() {
    let config_dir = invalid_config_dir();
    let output = cue(config_dir.path())
        .args(["config", "--path"])
        .output()
        .expect("run cue config --path");

    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("cue.toml"));
}

#[test]
fn model_operations_propagate_configuration_errors() {
    let config_dir = invalid_config_dir();
    let output = cue(config_dir.path())
        .args(["models", "list"])
        .output()
        .expect("run cue models list");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("configuration file is not valid TOML"),
        "{output:?}"
    );
}

#[test]
fn stream_requires_summary() {
    let config_dir = tempfile::tempdir().unwrap();
    let output = cue(config_dir.path())
        .args(["--stream", "missing.mp4"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--stream requires --summary"),
        "{output:?}"
    );
}

#[test]
fn processing_output_controls_are_not_ignored_by_subcommands() {
    let config_dir = tempfile::tempdir().unwrap();
    let output = cue(config_dir.path())
        .args(["--summary", "transcribe", "missing.mp4"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("apply only to the default processing"),
        "{output:?}"
    );
}

#[test]
fn summary_without_a_gateway_fails_before_creating_outputs() {
    let config_dir = tempfile::tempdir().unwrap();
    let media_dir = tempfile::tempdir().unwrap();
    let media = empty_media(media_dir.path(), "lesson.mp4");
    let output_dir = media_dir.path().join("out");

    let output = cue(config_dir.path())
        .args(["--summary", "--output"])
        .arg(&output_dir)
        .arg(&media)
        .output()
        .expect("run cue with a requested summary");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = stderr(&output);
    assert!(stderr.contains("summary"), "{stderr}");
    assert!(stderr.contains("Stage: analyze"), "{stderr}");
    assert!(stderr.contains("gateway"), "{stderr}");
    assert!(!output_dir.exists(), "{output_dir:?} should not exist");
}

#[test]
fn summary_batch_with_a_missing_credential_fails_before_creating_outputs() {
    let config_dir = tempfile::tempdir().unwrap();
    write_llm_config(
        config_dir.path(),
        "  CUE_TEST_SUMMARY_CREDENTIAL_DEFINITELY_UNSET_4B87  ",
    );
    let media_dir = tempfile::tempdir().unwrap();
    empty_media(media_dir.path(), "one.mp4");
    empty_media(media_dir.path(), "two.mp3");
    let output_dir = media_dir.path().join("out");

    let output = cue(config_dir.path())
        .args(["--summary", "--output"])
        .arg(&output_dir)
        .arg(media_dir.path())
        .output()
        .expect("run cue summary batch");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = stderr(&output);
    assert!(stderr.contains("Stage: analyze"), "{stderr}");
    assert!(
        stderr.contains("CUE_TEST_SUMMARY_CREDENTIAL_DEFINITELY_UNSET_4B87 is not set"),
        "{stderr}"
    );
    assert!(stderr.contains("api_key_env = \"\""), "{stderr}");
    assert!(!output_dir.exists(), "{output_dir:?} should not exist");
}

#[test]
fn unauthenticated_summary_configuration_passes_preflight() {
    let config_dir = tempfile::tempdir().unwrap();
    write_llm_config(config_dir.path(), "   ");
    let media_dir = tempfile::tempdir().unwrap();
    let media = empty_media(media_dir.path(), "lesson.mp4");

    let output = cue(config_dir.path())
        .arg("--summary")
        .arg(&media)
        .output()
        .expect("run cue with an unauthenticated gateway");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = stderr(&output);
    assert!(stderr.contains("Stage: inspect"), "{stderr}");
    assert!(
        !stderr.contains("cannot produce the requested summary"),
        "{stderr}"
    );
}

#[test]
fn available_summary_credential_passes_preflight() {
    let config_dir = tempfile::tempdir().unwrap();
    write_llm_config(config_dir.path(), "  CUE_TEST_SUMMARY_AVAILABLE  ");
    let media_dir = tempfile::tempdir().unwrap();
    let media = empty_media(media_dir.path(), "lesson.mp4");

    let output = cue(config_dir.path())
        .env("CUE_TEST_SUMMARY_AVAILABLE", "secret")
        .arg("--summary")
        .arg(&media)
        .output()
        .expect("run cue with an available credential");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = stderr(&output);
    assert!(stderr.contains("Stage: inspect"), "{stderr}");
    assert!(
        !stderr.contains("cannot produce the requested summary"),
        "{stderr}"
    );
}

#[test]
fn empty_summary_credential_fails_preflight() {
    let config_dir = tempfile::tempdir().unwrap();
    write_llm_config(config_dir.path(), "CUE_TEST_SUMMARY_EMPTY");
    let media_dir = tempfile::tempdir().unwrap();
    let media = empty_media(media_dir.path(), "lesson.mp4");
    let output_dir = media_dir.path().join("out");

    let output = cue(config_dir.path())
        .env("CUE_TEST_SUMMARY_EMPTY", "")
        .args(["--summary", "--output"])
        .arg(&output_dir)
        .arg(&media)
        .output()
        .expect("run cue with an empty credential");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = stderr(&output);
    assert!(stderr.contains("Stage: analyze"), "{stderr}");
    assert!(
        stderr.contains("CUE_TEST_SUMMARY_EMPTY is not set"),
        "{stderr}"
    );
    assert!(!output_dir.exists(), "{output_dir:?} should not exist");
}

#[test]
fn input_resolution_errors_precede_summary_readiness_errors() {
    let config_dir = tempfile::tempdir().unwrap();
    let missing = config_dir.path().join("missing.mp4");

    let output = cue(config_dir.path())
        .arg("--summary")
        .arg(&missing)
        .output()
        .expect("run cue with a missing input");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = stderr(&output);
    assert!(stderr.contains("does not exist"), "{stderr}");
    assert!(!stderr.contains("no LLM gateway is configured"), "{stderr}");
}

#[test]
fn processing_without_summary_does_not_require_a_gateway() {
    let config_dir = tempfile::tempdir().unwrap();
    let media_dir = tempfile::tempdir().unwrap();
    let media = empty_media(media_dir.path(), "lesson.mp4");

    let output = cue(config_dir.path())
        .arg(&media)
        .output()
        .expect("run cue without a requested summary");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = stderr(&output);
    assert!(stderr.contains("Stage: inspect"), "{stderr}");
    assert!(!stderr.contains("no LLM gateway is configured"), "{stderr}");
}
