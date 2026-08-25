//! cue — turn video and audio files into transcripts, subtitles, and
//! descriptions.

mod cli;
mod commands;
mod events;
mod logging;
mod render;

use std::process::ExitCode;

use cli::{Command, Cue};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cue::parse();
    logging::init(cli.verbose);
    tracing::debug!("parsed CLI: {:?}", cli);

    let code = match cli.command {
        Some(Command::Doctor(args)) => commands::doctor::run(args).await,
        Some(Command::Transcribe(ref args)) => {
            commands::process::run_stopped(
                &cli,
                &args.files,
                Some(cue_core::PipelineStage::Transcribe),
            )
            .await
        }
        Some(Command::Models(args)) => commands::models::run(args).await,
        Some(Command::Config(args)) => commands::config_cmd::run(args),
        Some(Command::Cache(args)) => commands::cache_cmd::run(args.command),
        Some(Command::Skill(args)) => commands::skill::run(args).await,
        Some(Command::Correct(args)) => commands::correct::run(args),
        None => commands::process::run(&cli).await,
    };

    ExitCode::from(code as u8)
}
