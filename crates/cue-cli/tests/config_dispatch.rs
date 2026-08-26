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
