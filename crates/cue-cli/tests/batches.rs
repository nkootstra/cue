use std::path::Path;
use std::process::{Command, Output};

use serde_json::json;

struct TestEnvironment {
    _root: tempfile::TempDir,
    cwd: std::path::PathBuf,
    state: std::path::PathBuf,
    data: std::path::PathBuf,
    cache: std::path::PathBuf,
    config: std::path::PathBuf,
}

impl TestEnvironment {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create isolated test root");
        let cwd = root.path().join("cwd");
        let state = root.path().join("state");
        let data = root.path().join("data");
        let cache = root.path().join("cache");
        let config = root.path().join("config");
        for directory in [&cwd, &state, &data, &cache, &config] {
            std::fs::create_dir(directory).expect("create isolated test directory");
        }
        Self {
            _root: root,
            cwd,
            state,
            data,
            cache,
            config,
        }
    }

    fn cue(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cue"));
        command
            .current_dir(&self.cwd)
            .env("CUE_STATE_DIR", &self.state)
            .env("CUE_DATA_DIR", &self.data)
            .env("CUE_CACHE_DIR", &self.cache)
            .env("CUE_CONFIG_DIR", &self.config)
            .env("HOME", self._root.path());
        command
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn scope_key(cwd: &Path) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in canonical(cwd).as_os_str().to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("cwd-{hash:016x}")
}

fn state(status: &str, attempt: u32) -> serde_json::Value {
    match status {
        "pending" => json!({ "status": "pending" }),
        "running" => json!({
            "status": "running",
            "attempt": {
                "number": attempt,
                "started_at_ms": 100,
                "finished_at_ms": null
            }
        }),
        "complete" => json!({
            "status": "complete",
            "attempt": {
                "number": attempt,
                "started_at_ms": 100,
                "finished_at_ms": 200
            }
        }),
        "failed" => json!({
            "status": "failed",
            "attempt": {
                "number": attempt,
                "started_at_ms": 100,
                "finished_at_ms": 200
            },
            "failure": {
                "stage": "transcribe",
                "summary": "provider request failed",
                "remedy": "check the provider and resume"
            }
        }),
        other => panic!("unsupported fixture status: {other}"),
    }
}

struct Journal<'a> {
    id: &'a str,
    cwd: &'a Path,
    created_at_ms: u64,
    mode: &'a str,
    statuses: &'a [&'a str],
}

fn write_journal(environment: &TestEnvironment, journal: Journal<'_>) -> std::path::PathBuf {
    let canonical_cwd = canonical(journal.cwd);
    let directory = environment
        .state
        .join("batches")
        .join(scope_key(&canonical_cwd));
    std::fs::create_dir_all(&directory).expect("create batch scope");
    let items = journal
        .statuses
        .iter()
        .enumerate()
        .map(|(position, status)| {
            let source = canonical_cwd.join(format!("lesson-{position}.mp4"));
            json!({
                "position": position,
                "source": source,
                "workspace": canonical_cwd.join(format!("lesson-{position}.cue")),
                "published_base": canonical_cwd.join(format!("lesson-{position}")),
                "state": state(status, 2)
            })
        })
        .collect::<Vec<_>>();
    let record = json!({
        "schema_version": 1,
        "id": journal.id,
        "cue_version": env!("CARGO_PKG_VERSION"),
        "created_at_ms": journal.created_at_ms,
        "updated_at_ms": journal.created_at_ms,
        "cwd": canonical_cwd,
        "intent": {
            "mode": journal.mode,
            "language": null,
            "subtitle_formats": ["srt"],
            "summary": false,
            "stream": false,
            "corrections": null
        },
        "items": items
    });
    let path = directory.join(format!("{}.json", journal.id));
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&record).expect("serialize batch fixture"),
    )
    .expect("write batch fixture");
    path
}

#[test]
fn resume_without_an_incomplete_batch_is_an_informative_no_op() {
    let environment = TestEnvironment::new();

    let output = environment
        .cue()
        .arg("resume")
        .output()
        .expect("run cue resume");

    assert!(output.status.success(), "{output:?}");
    assert!(
        stdout(&output).contains("No incomplete batch to resume"),
        "{output:?}"
    );
}

#[test]
fn batches_list_is_newest_first_and_keeps_corrupt_neighbors_visible() {
    let environment = TestEnvironment::new();
    write_journal(
        &environment,
        Journal {
            id: "batch-older",
            cwd: &environment.cwd,
            created_at_ms: 10,
            mode: "full",
            statuses: &["complete"],
        },
    );
    write_journal(
        &environment,
        Journal {
            id: "batch-incomplete",
            cwd: &environment.cwd,
            created_at_ms: 15,
            mode: "full",
            statuses: &["failed"],
        },
    );
    let newest = write_journal(
        &environment,
        Journal {
            id: "batch-newer",
            cwd: &environment.cwd,
            created_at_ms: 20,
            mode: "transcript-only",
            statuses: &["running", "failed"],
        },
    );
    std::fs::write(newest.with_file_name("corrupt.json"), b"not JSON")
        .expect("write corrupt neighbor");
    std::fs::write(environment.config.join("cue.toml"), b"invalid TOML = [")
        .expect("write invalid config");

    let output = environment
        .cue()
        .args(["batches", "list"])
        .output()
        .expect("run cue batches list");

    assert!(output.status.success(), "{output:?}");
    let output = stdout(&output);
    let newer = output.find("batch-newer").expect("show newer batch");
    let older = output.find("batch-older").expect("show older batch");
    assert!(newer < older, "{output}");
    assert!(output.contains("interrupted"), "{output}");
    assert!(output.contains("complete"), "{output}");
    assert!(output.contains("batch-incomplete  incomplete"), "{output}");
    assert!(output.contains("corrupt.json"), "{output}");
    assert!(output.contains("unreadable"), "{output}");
}

#[test]
fn batches_show_reports_recorded_intent_and_safe_ordered_item_details() {
    let environment = TestEnvironment::new();
    let path = write_journal(
        &environment,
        Journal {
            id: "batch-details",
            cwd: &environment.cwd,
            created_at_ms: 30,
            mode: "transcript-only",
            statuses: &["complete", "failed", "running"],
        },
    );
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["intent"]["language"] = json!("nl");
    record["items"][1]["state"]["failure"]["summary"] =
        json!("provider failed without private response content");
    record["items"][1]["state"]["failure"]["remedy"] =
        json!("check provider connectivity and resume");
    record["items"].as_array_mut().unwrap().reverse();
    std::fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    std::fs::write(
        environment.config.join("cue.toml"),
        b"api_key = 'secret-token'\ntranscript = 'private spoken words'\nnot valid = [",
    )
    .expect("write invalid sensitive config");

    let output = environment
        .cue()
        .args(["batches", "show", "batch-details"])
        .output()
        .expect("run cue batches show");

    assert!(output.status.success(), "{output:?}");
    let output = stdout(&output);
    assert!(output.contains("Mode: transcript-only"), "{output}");
    assert!(output.contains("Language: nl"), "{output}");
    assert!(output.contains("Status: interrupted"), "{output}");
    assert!(
        output.contains("Next action: cue resume batch-details"),
        "{output}"
    );
    assert!(output.contains("attempt 2"), "{output}");
    assert!(output.contains("verification required"), "{output}");
    assert!(output.contains("provider failed without private response content"));
    assert!(output.contains("check provider connectivity and resume"));
    let first = output.find("lesson-0.mp4").unwrap();
    let second = output.find("lesson-1.mp4").unwrap();
    let third = output.find("lesson-2.mp4").unwrap();
    assert!(first < second && second < third, "{output}");
    assert!(!output.contains("secret-token"), "{output}");
    assert!(!output.contains("private spoken words"), "{output}");
}

#[test]
fn explicit_complete_batch_path_is_a_no_op_before_configuration_loading() {
    let environment = TestEnvironment::new();
    let other_cwd = environment._root.path().join("other-cwd");
    std::fs::create_dir(&other_cwd).unwrap();
    let path = write_journal(
        &environment,
        Journal {
            id: "batch-complete",
            cwd: &other_cwd,
            created_at_ms: 40,
            mode: "full",
            statuses: &["complete"],
        },
    );
    std::fs::write(environment.config.join("cue.toml"), b"invalid TOML = [").unwrap();

    let output = environment
        .cue()
        .arg("resume")
        .arg(&path)
        .output()
        .expect("run explicit cue resume");

    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("batch-complete is already complete"));
    assert!(
        !stderr(&output).contains("configuration file"),
        "{output:?}"
    );
}

#[test]
fn explicit_external_journal_is_updated_in_place_during_resume() {
    let environment = TestEnvironment::new();
    let other_cwd = environment._root.path().join("external-cwd");
    std::fs::create_dir(&other_cwd).unwrap();
    let canonical_path = write_journal(
        &environment,
        Journal {
            id: "batch-external",
            cwd: &other_cwd,
            created_at_ms: 45,
            mode: "transcript-only",
            statuses: &["pending"],
        },
    );
    let external = environment._root.path().join("external-batch.json");
    std::fs::rename(canonical_path, &external).unwrap();

    let resumed = environment
        .cue()
        .arg("resume")
        .arg(&external)
        .output()
        .unwrap();
    assert_eq!(resumed.status.code(), Some(1), "{resumed:?}");
    let shown = environment
        .cue()
        .args(["batches", "show"])
        .arg(&external)
        .output()
        .unwrap();
    assert!(shown.status.success(), "{shown:?}");
    assert!(stdout(&shown).contains("— missing, attempt 1"), "{shown:?}");
    let by_id = environment
        .cue()
        .args(["batches", "show", "batch-external"])
        .output()
        .unwrap();
    assert_eq!(by_id.status.code(), Some(1), "{by_id:?}");
}

#[test]
fn resume_rejects_processing_overrides_instead_of_ignoring_them() {
    let environment = TestEnvironment::new();
    let cases: &[&[&str]] = &[
        &["--language", "nl", "resume"],
        &["--output", "elsewhere", "resume"],
        &["--format", "vtt", "resume"],
        &["--summary", "resume"],
        &["--summary", "--stream", "resume"],
        &["--recursive", "resume"],
        &["--corrections", "terms.md", "resume"],
    ];

    for args in cases {
        let output = environment
            .cue()
            .args(*args)
            .output()
            .expect("run cue resume with invalid processing override");
        assert_eq!(output.status.code(), Some(1), "args={args:?} {output:?}");
        assert!(
            stderr(&output).contains("cue resume accepts only --jobs"),
            "args={args:?} {output:?}"
        );
    }

    let allowed = environment
        .cue()
        .args(["--jobs", "3", "resume"])
        .output()
        .unwrap();
    assert!(allowed.status.success(), "{allowed:?}");
}

#[test]
fn explicit_invalid_recovery_targets_fail_actionably() {
    let environment = TestEnvironment::new();

    let missing = environment
        .cue()
        .args(["resume", "batch-missing"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1), "{missing:?}");
    assert!(stderr(&missing).contains("was not found"), "{missing:?}");

    let malformed = environment.cwd.join("malformed.json");
    std::fs::write(&malformed, b"not a recovery record").unwrap();
    let output = environment
        .cue()
        .arg("resume")
        .arg(&malformed)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr(&output).contains("could not parse batch recovery state"));

    let unsupported = write_journal(
        &environment,
        Journal {
            id: "batch-unsupported",
            cwd: &environment.cwd,
            created_at_ms: 50,
            mode: "full",
            statuses: &["failed"],
        },
    );
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&unsupported).unwrap()).unwrap();
    record["schema_version"] = json!(99);
    std::fs::write(&unsupported, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    let output = environment
        .cue()
        .arg("resume")
        .arg(&unsupported)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr(&output).contains("unsupported batch recovery schema"));

    let directory = environment.cwd.join("record-directory");
    std::fs::create_dir(&directory).unwrap();
    let output = environment
        .cue()
        .arg("resume")
        .arg(&directory)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr(&output).contains("not a regular file"));

    #[cfg(unix)]
    {
        let valid = write_journal(
            &environment,
            Journal {
                id: "batch-link-target",
                cwd: &environment.cwd,
                created_at_ms: 51,
                mode: "full",
                statuses: &["complete"],
            },
        );
        let link = environment.cwd.join("linked-record.json");
        std::os::unix::fs::symlink(valid, &link).unwrap();
        let output = environment.cue().arg("resume").arg(&link).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(stderr(&output).contains("not a regular file"));
    }
}

#[test]
fn locked_resume_target_reports_busy_before_loading_configuration() {
    let environment = TestEnvironment::new();
    let path = write_journal(
        &environment,
        Journal {
            id: "batch-busy",
            cwd: &environment.cwd,
            created_at_ms: 60,
            mode: "transcript-only",
            statuses: &["failed"],
        },
    );
    std::fs::write(environment.config.join("cue.toml"), b"invalid TOML = [").unwrap();
    let lock_path = path.with_file_name(format!(
        ".{}.lock",
        path.file_name().unwrap().to_string_lossy()
    ));
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .unwrap();
    fs4::FileExt::try_lock(&lock).expect("hold batch lock");

    let output = environment
        .cue()
        .args(["resume", "batch-busy"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        stderr(&output).contains("already being processed"),
        "{output:?}"
    );
    assert!(
        !stderr(&output).contains("configuration file"),
        "{output:?}"
    );
}

#[test]
fn default_resume_selects_newest_incomplete_in_scope_and_id_selects_older() {
    let environment = TestEnvironment::new();
    write_journal(
        &environment,
        Journal {
            id: "batch-old-pending",
            cwd: &environment.cwd,
            created_at_ms: 70,
            mode: "transcript-only",
            statuses: &["pending"],
        },
    );
    write_journal(
        &environment,
        Journal {
            id: "batch-new-pending",
            cwd: &environment.cwd,
            created_at_ms: 80,
            mode: "transcript-only",
            statuses: &["pending"],
        },
    );

    let resumed = environment.cue().arg("resume").output().unwrap();
    assert_eq!(resumed.status.code(), Some(1), "{resumed:?}");
    let newest = environment
        .cue()
        .args(["batches", "show", "batch-new-pending"])
        .output()
        .unwrap();
    assert!(
        stdout(&newest).contains("— missing, attempt 1"),
        "{newest:?}"
    );
    let older = environment
        .cue()
        .args(["batches", "show", "batch-old-pending"])
        .output()
        .unwrap();
    assert!(
        stdout(&older).contains("— pending, not attempted"),
        "{older:?}"
    );

    let resumed = environment
        .cue()
        .args(["resume", "batch-old-pending"])
        .output()
        .unwrap();
    assert_eq!(resumed.status.code(), Some(1), "{resumed:?}");
    let older = environment
        .cue()
        .args(["batches", "show", "batch-old-pending"])
        .output()
        .unwrap();
    assert!(stdout(&older).contains("— missing, attempt 1"), "{older:?}");
}

#[test]
fn implicit_resume_never_crosses_working_directory_scope() {
    let environment = TestEnvironment::new();
    let other_cwd = environment._root.path().join("other-scope");
    std::fs::create_dir(&other_cwd).unwrap();
    write_journal(
        &environment,
        Journal {
            id: "batch-other-scope",
            cwd: &other_cwd,
            created_at_ms: 90,
            mode: "transcript-only",
            statuses: &["pending"],
        },
    );

    let output = environment.cue().arg("resume").output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("No incomplete batch to resume"));
}

#[test]
fn list_and_show_report_a_locked_running_batch_as_active() {
    let environment = TestEnvironment::new();
    let path = write_journal(
        &environment,
        Journal {
            id: "batch-active",
            cwd: &environment.cwd,
            created_at_ms: 100,
            mode: "full",
            statuses: &["running"],
        },
    );
    let lock_path = path.with_file_name(format!(
        ".{}.lock",
        path.file_name().unwrap().to_string_lossy()
    ));
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .unwrap();
    fs4::FileExt::try_lock(&lock).expect("hold batch lock");

    let listed = environment
        .cue()
        .args(["batches", "list"])
        .output()
        .unwrap();
    assert!(listed.status.success(), "{listed:?}");
    assert!(
        stdout(&listed).contains("batch-active  active"),
        "{listed:?}"
    );

    let shown = environment
        .cue()
        .args(["batches", "show", "batch-active"])
        .output()
        .unwrap();
    assert!(shown.status.success(), "{shown:?}");
    assert!(stdout(&shown).contains("Status: active"), "{shown:?}");
}

#[test]
fn resume_dispatches_both_recorded_processing_modes() {
    let environment = TestEnvironment::new();
    for (id, mode, timestamp) in [
        ("batch-full-mode", "full", 110),
        ("batch-transcript-mode", "transcript-only", 120),
    ] {
        write_journal(
            &environment,
            Journal {
                id,
                cwd: &environment.cwd,
                created_at_ms: timestamp,
                mode,
                statuses: &["pending"],
            },
        );
        let resumed = environment.cue().args(["resume", id]).output().unwrap();
        assert_eq!(resumed.status.code(), Some(1), "mode={mode} {resumed:?}");
        let shown = environment
            .cue()
            .args(["batches", "show", id])
            .output()
            .unwrap();
        let shown = stdout(&shown);
        assert!(shown.contains(&format!("Mode: {mode}")), "{shown}");
        assert!(shown.contains("— missing, attempt 1"), "{shown}");
    }
}

#[test]
fn public_help_exposes_resume_and_batch_inspection_commands() {
    let environment = TestEnvironment::new();
    let root = environment.cue().arg("-h").output().unwrap();
    assert!(root.status.success(), "{root:?}");
    let root = stdout(&root);
    assert!(root.contains("resume"), "{root}");
    assert!(root.contains("batches"), "{root}");

    let resume = environment.cue().args(["resume", "-h"]).output().unwrap();
    assert!(resume.status.success(), "{resume:?}");
    let resume = stdout(&resume);
    assert!(resume.contains("[TARGET]"), "{resume}");
    assert!(resume.contains("--jobs"), "{resume}");

    let batches = environment.cue().args(["batches", "-h"]).output().unwrap();
    assert!(batches.status.success(), "{batches:?}");
    let batches = stdout(&batches);
    assert!(batches.contains("list"), "{batches}");
    assert!(batches.contains("show"), "{batches}");
}

#[allow(dead_code)]
fn canonical(path: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).expect("canonicalize fixture path")
}
