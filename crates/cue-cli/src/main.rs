//! cue — turn video and audio files into transcripts, subtitles, and
//! descriptions.

mod cli;
mod commands;
mod corrections;
mod events;
mod logging;
mod render;

use std::process::ExitCode;

use cli::{Command, Cue, SubtitlesCommand};
use cue_core::config::{load_user_config, resolve};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cue::parse();
    logging::init(cli.verbose);
    tracing::debug!("parsed CLI: {:?}", cli);

    let code = match dispatch(cli).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    };

    ExitCode::from(code as u8)
}

async fn dispatch(mut cli: Cue) -> cue_core::Result<i32> {
    match cli.command.take() {
        Some(Command::Doctor(args)) => {
            let config = load_resolved_config()?;
            Ok(commands::doctor::run(args, &config).await)
        }
        Some(Command::Transcribe(args)) => {
            let config = load_resolved_config()?;
            Ok(commands::process::run_mode(
                &cli,
                &args.paths,
                commands::process::ProcessMode::TranscriptOnly,
                &config,
            )
            .await)
        }
        Some(Command::Models(args)) => match args.command {
            None => Ok(commands::models::print_help()),
            Some(command) => {
                let config = load_resolved_config()?;
                Ok(commands::models::run(command, &config).await)
            }
        },
        Some(Command::Config(args)) => Ok(commands::config_cmd::run(args)),
        Some(Command::Cache(args)) => Ok(commands::cache_cmd::run(args.command)),
        Some(Command::Skill(args)) => Ok(commands::skill::run(args).await),
        Some(Command::Correct(args)) => {
            let config = load_resolved_config()?;
            Ok(commands::correct::run(
                args,
                cli.corrections.as_deref(),
                &config,
            ))
        }
        Some(Command::Lexicon(args)) => Ok(commands::lexicon::run(args)),
        Some(Command::Review(args)) => Ok(commands::review::run(args, cli.corrections.as_deref())),
        Some(Command::Subtitles(args)) => match args.command {
            Some(SubtitlesCommand::Check(args)) => {
                let config = load_resolved_config()?;
                Ok(commands::subtitles::check(
                    args,
                    cli.corrections.as_deref(),
                    &config,
                ))
            }
            None => Ok(commands::subtitles::print_help()),
        },
        None => {
            let config = load_resolved_config()?;
            Ok(commands::process::run(&cli, &config).await)
        }
    }
}

fn load_resolved_config() -> cue_core::Result<cue_core::Config> {
    let user = load_user_config()?;
    let config = resolve(&[&user])?;
    tracing::debug!(?config, "resolved configuration");
    Ok(config)
}
