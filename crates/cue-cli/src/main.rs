//! cue — turn video and audio files into transcripts, subtitles, and
//! descriptions.

mod batch_recovery;
mod cli;
mod commands;
mod corrections;
mod events;
mod logging;
mod render;
mod run_contract;
mod verification;

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
    if matches!(&cli.command, Some(Command::Resume(_)))
        && (cli.language.is_some()
            || cli.output.is_some()
            || !cli.format.is_empty()
            || cli.summary
            || cli.stream
            || cli.recursive
            || cli.corrections.is_some()
            || !cli.paths.is_empty())
    {
        return Err(cue_core::CueError::general(
            "cue resume accepts only --jobs as a processing-policy override",
        )
        .remedy("remove media paths and artifact-affecting processing flags"));
    }
    if cli.stream && !cli.summary {
        return Err(cue_core::CueError::general("--stream requires --summary"));
    }
    if cli.command.is_some() && (cli.summary || cli.stream || !cli.format.is_empty()) {
        return Err(cue_core::CueError::general(
            "--format, --summary, and --stream apply only to the default processing command",
        ));
    }
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
                cli.output.as_deref().map(std::path::Path::new),
                &config,
            ))
        }
        Some(Command::Lexicon(args)) => Ok(commands::lexicon::run(
            args,
            cli.output.as_deref().map(std::path::Path::new),
        )),
        Some(Command::Review(args)) => Ok(commands::review::run(
            args,
            cli.corrections.as_deref(),
            cli.output.as_deref().map(std::path::Path::new),
        )),
        Some(Command::Subtitles(args)) => match args.command {
            Some(SubtitlesCommand::Check(args)) => {
                let config = load_resolved_config()?;
                Ok(commands::subtitles::check(
                    args,
                    cli.corrections.as_deref(),
                    &config,
                    cli.output.as_deref().map(std::path::Path::new),
                ))
            }
            None => Ok(commands::subtitles::print_help()),
        },
        Some(Command::Verify(args)) => Ok(commands::verify::run(
            args,
            cli.output.as_deref().map(std::path::Path::new),
        )),
        Some(Command::Resume(args)) => {
            commands::resume::run(args.target.as_deref(), cli.jobs, load_resolved_config).await
        }
        Some(Command::Batches(args)) => commands::batches::run(args.command),
        None => {
            let mut config = load_resolved_config()?;
            if !cli.format.is_empty() {
                let mut formats = Vec::new();
                for format in &cli.format {
                    if !formats.contains(format) {
                        formats.push(*format);
                    }
                }
                config.subtitles.formats = formats;
            }
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
