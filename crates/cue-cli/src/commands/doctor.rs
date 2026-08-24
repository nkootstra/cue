//! `cue doctor` — inspect the local environment, optionally fixing it.

use cue_media::check_environment;
use cue_media::ToolStatus;

use crate::cli::DoctorArgs;
use crate::render::{println_line, tool_line};

/// Run environment checks and render them. Returns the process exit code.
pub async fn run(args: DoctorArgs) -> i32 {
    println_line("Checking local environment...\n");
    let env = check_environment().await;

    for report in &env.reports {
        println_line(&tool_line(report));
    }

    let fixed_ok = if args.fix {
        Some(try_fix(&env).await)
    } else {
        None
    };

    match (env.all_ok(), fixed_ok) {
        (true, _) => {
            println_line("\nAll required tools are available.");
            0
        }
        (false, Some(true)) => {
            println_line("\nTranscription environment is ready. Re-run `cue doctor`");
            println_line("for a full status.");
            0
        }
        (_, _) => {
            println_line("\nSome tools are missing. Install FFmpeg (and Python 3.10+) to");
            println_line("process media files, or run `cue doctor --fix` to set up the");
            println_line("Python transcription environment.");
            1
        }
    }
}

/// Attempt to provision the Python transcription environment.
///
/// Returns true when the environment is usable afterwards.
async fn try_fix(env: &cue_media::checks::Environment) -> bool {
    let Some(python) = python_for_provisioning(env) else {
        println_line("\n--fix: no Python found; cannot provision the transcription");
        println_line("environment. Install Python 3.10+ first.");
        return false;
    };

    let Some(data_dir) = cue_transcription::env::data_dir() else {
        println_line("\n--fix: could not determine cue's data directory (set HOME).");
        return false;
    };

    let venv_dir = cue_transcription::provision::venv_dir(&data_dir);
    println_line("\n--fix: provisioning the transcription environment...");
    println_line(&format!("  venv: {}", venv_dir.display()));
    println_line("  installing pinned faster-whisper dependencies");

    match cue_transcription::provision::provision(&data_dir, &python, false).await {
        Ok(cue_transcription::provision::ProvisionAction::AlreadyProvisioned) => {
            println_line("  already provisioned — nothing to do");
            true
        }
        Ok(cue_transcription::provision::ProvisionAction::Created) => {
            println_line("  done");
            true
        }
        Err(err) => {
            eprintln!("{err}");
            false
        }
    }
}

/// Prefer the checked system python for creating the venv.
fn python_for_provisioning(env: &cue_media::checks::Environment) -> Option<std::path::PathBuf> {
    let report = env.python()?;
    match &report.status {
        ToolStatus::Available { path, .. } => Some(std::path::PathBuf::from(path)),
        _ => cue_media::tools::find_on_path("python3"),
    }
}
