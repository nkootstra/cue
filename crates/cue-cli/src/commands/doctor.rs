//! `cue doctor` — inspect the local environment.

use cue_media::check_environment;

use crate::cli::DoctorArgs;
use crate::render::{println_line, tool_line};

/// Run environment checks and render them. Returns the process exit code.
pub async fn run(args: DoctorArgs) -> i32 {
    println_line("Checking local environment...\n");
    let env = check_environment().await;

    for report in &env.reports {
        println_line(&tool_line(report));
    }

    if args.fix {
        println_line("\n--fix is not implemented yet; automatic setup arrives with");
        println_line("the transcription milestone.");
    }

    if env.all_ok() {
        println_line("\nAll required tools are available.");
        0
    } else {
        println_line("\nSome tools are missing. Install FFmpeg (and Python 3.10+) to");
        println_line("process media files.");
        1
    }
}
