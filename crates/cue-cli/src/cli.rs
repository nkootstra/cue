//! CLI declaration.
//!
//! This struct is the single source of truth for parsing, help, and (via
//! the emitted usage spec) completions and docs.

use std::path::PathBuf;
use usage::{Args, Cli, Subcommands};

/// Turn video and audio files into transcripts, subtitles, and descriptions.
///
/// Processing runs locally; optional AI analysis uses your configured LLM
/// gateway.
#[derive(Debug, Cli)]
#[usage(bin = "cue", version, unknown_flags = "error")]
pub struct Cue {
    /// Print diagnostic logging to stderr
    #[usage(short = 'v', long, global)]
    pub verbose: bool,

    /// Language of the media content (e.g. "en"); auto-detected when unset
    #[usage(long, global)]
    pub language: Option<String>,

    /// Directory for generated outputs
    #[usage(long, global)]
    pub output: Option<String>,

    /// Search directory inputs recursively
    #[usage(short = 'r', long, global)]
    pub recursive: bool,

    /// Media files or directories to process
    pub paths: Vec<PathBuf>,

    /// What to do (defaults to processing the given files)
    #[usage(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommands)]
pub enum Command {
    /// Check the local environment for required and optional tools
    Doctor(DoctorArgs),
    /// Transcribe files or directories to a canonical transcript without subtitles or AI
    Transcribe(TranscribeArgs),
    /// Manage transcription and normalization models
    Models(ModelsArgs),
    /// Show resolved configuration and its sources
    Config(ConfigArgs),
    /// Manage the processing cache
    Cache(CacheArgs),
    /// Install the transcribe agent skill for AI agents
    Skill(SkillArgs),
    /// Apply transcript corrections from a manifest to cue outputs
    Correct(CorrectArgs),
}

/// Transcribe files or directories
#[derive(Debug, Args)]
pub struct TranscribeArgs {
    /// Media files or directories to transcribe
    pub paths: Vec<PathBuf>,
}

/// Check the local environment
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Attempt to fix detected problems (e.g. install the Python worker)
    #[usage(long)]
    pub fix: bool,
}

/// Manage transcription and normalization models
#[derive(Debug, Args)]
pub struct ModelsArgs {
    /// Which model action to run
    #[usage(subcommand)]
    pub command: Option<ModelsCommand>,
}

#[derive(Debug, Subcommands)]
pub enum ModelsCommand {
    /// List models relevant to cue and their availability
    List,
    /// Verify configured models exist and load correctly
    Check,
    /// Create the S1 normalization model in Ollama from cue's Modelfile
    Install(InstallModelArgs),
}

/// Create a model in Ollama
#[derive(Debug, Args)]
pub struct InstallModelArgs {
    /// Model family to install
    pub model: String,
}

/// Show resolved configuration and its sources
#[derive(Debug, Args)]
pub struct ConfigArgs {
    /// Show the path of the user configuration file
    #[usage(long)]
    pub path: bool,
}

/// Manage the processing cache
#[derive(Debug, Args)]
pub struct CacheArgs {
    /// Which cache action to run
    #[usage(subcommand)]
    pub command: Option<CacheCommand>,
}

/// Manage the transcribe agent skill
#[derive(Debug, Args)]
pub struct SkillArgs {
    /// Which skill action to run
    #[usage(subcommand)]
    pub command: Option<SkillCommand>,
}

/// Apply transcript corrections from a manifest
#[derive(Debug, Args)]
pub struct CorrectArgs {
    /// A cue output directory (e.g. "video.cue/") or a media file whose
    /// sibling ".cue/" directory exists
    pub output: String,
    /// Corrections manifest file (default: corrections.md in the output
    /// directory, then in its parent)
    #[usage(long)]
    pub corrections: Option<String>,
    /// Preview what would change without writing any files
    #[usage(long)]
    pub dry_run: bool,
}

#[derive(Debug, Subcommands)]
pub enum SkillCommand {
    /// Install the transcribe skill for AI agents (proxies `npx skills add`)
    Install(SkillInstallArgs),
}

/// Install the transcribe skill
#[derive(Debug, Args)]
pub struct SkillInstallArgs {
    /// Skill source repo in owner/repo form
    #[usage(long, default = "nkootstra/cue")]
    pub repo: String,
    /// Target agent (e.g. "opencode", "claude-code"); auto-detect when unset
    #[usage(long)]
    pub agent: Option<String>,
    /// Opt out of the skills CLI's anonymous telemetry
    #[usage(long)]
    pub no_telemetry: bool,
}

#[derive(Debug, Subcommands)]
pub enum CacheCommand {
    /// Print the cache directory
    Dir,
    /// Remove all cached intermediate artifacts
    Clear,
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::path::PathBuf;

    use super::*;

    fn parse_args(args: &[&str]) -> Cue {
        let os: Vec<OsString> = args.iter().map(OsString::from).collect();
        let refs: Vec<&OsStr> = os.iter().map(|s| s.as_os_str()).collect();
        Cue::try_parse_from(&refs).expect("parse failed")
    }

    #[test]
    fn bare_paths_become_the_default_pipeline() {
        let cli = parse_args(&["cue", "--verbose", "./video.mp4"]);
        assert!(cli.verbose);
        assert_eq!(cli.paths, vec![PathBuf::from("./video.mp4")]);
        assert!(cli.command.is_none());
        assert!(cli.language.is_none());
    }

    #[test]
    fn doctor_takes_fix_flag() {
        let cli = parse_args(&["cue", "doctor", "--fix"]);
        match cli.command.unwrap() {
            Command::Doctor(args) => assert!(args.fix),
            other => panic!("wrong command: {other:?}"),
        }
    }

    #[test]
    fn transcribe_takes_paths() {
        let cli = parse_args(&["cue", "transcribe", "a.mp4", "b.mp3"]);
        match cli.command.unwrap() {
            Command::Transcribe(args) => {
                assert_eq!(
                    args.paths,
                    vec![PathBuf::from("a.mp4"), PathBuf::from("b.mp3")]
                )
            }
            other => panic!("wrong command: {other:?}"),
        }
    }

    #[test]
    fn root_flags_apply_to_transcribe() {
        // --language/--output are root flags; the subcommand only carries
        // files, so global options stay on the root struct.
        let cli = parse_args(&["cue", "--language", "de", "transcribe", "a.mp4"]);
        assert_eq!(cli.language.as_deref(), Some("de"));
        match cli.command.unwrap() {
            Command::Transcribe(args) => assert_eq!(args.paths, vec![PathBuf::from("a.mp4")]),
            other => panic!("wrong command: {other:?}"),
        }
    }

    #[test]
    fn models_install_takes_model_name() {
        let cli = parse_args(&["cue", "models", "install", "s1"]);
        match cli.command.unwrap() {
            Command::Models(models) => match models.command.unwrap() {
                ModelsCommand::Install(args) => assert_eq!(args.model, "s1"),
                other => panic!("wrong subcommand: {other:?}"),
            },
            other => panic!("wrong command: {other:?}"),
        }
    }

    #[test]
    fn nested_cache_dir_parses() {
        let cli = parse_args(&["cue", "cache", "dir"]);
        match cli.command.unwrap() {
            Command::Cache(cache) => match cache.command.unwrap() {
                CacheCommand::Dir => {}
                other => panic!("wrong subcommand: {other:?}"),
            },
            other => panic!("wrong command: {other:?}"),
        }
    }

    #[test]
    fn root_flags_reach_processing_options() {
        let cli = parse_args(&[
            "cue",
            "--language",
            "en",
            "--output",
            "./result",
            "clip.mp4",
        ]);
        assert_eq!(cli.language.as_deref(), Some("en"));
        assert_eq!(cli.output.as_deref(), Some("./result"));
    }

    #[test]
    fn empty_invocation_is_valid() {
        // All fields optional so we can print our own help.
        let cli = parse_args(&["cue"]);
        assert!(cli.paths.is_empty());
        assert!(cli.command.is_none());
    }

    #[test]
    fn recursive_is_global_before_or_after_transcribe() {
        let before = parse_args(&["cue", "-r", "transcribe", "media"]);
        assert!(before.recursive);
        match before.command.unwrap() {
            Command::Transcribe(args) => assert_eq!(args.paths, [PathBuf::from("media")]),
            other => panic!("wrong command: {other:?}"),
        }

        let after = parse_args(&["cue", "transcribe", "--recursive", "media"]);
        assert!(after.recursive);
        match after.command.unwrap() {
            Command::Transcribe(args) => assert_eq!(args.paths, [PathBuf::from("media")]),
            other => panic!("wrong command: {other:?}"),
        }
    }

    #[test]
    fn emitted_spec_names_paths_and_recursive() {
        let kdl = Cue::to_kdl();
        assert!(kdl.contains("[PATHS]"), "{kdl}");
        assert!(kdl.contains("flag \"-r --recursive\""), "{kdl}");
    }

    #[test]
    fn unknown_flag_is_rejected() {
        let os: Vec<OsString> = vec![OsString::from("cue"), OsString::from("--bogus")];
        let refs: Vec<&OsStr> = os.iter().map(|s| s.as_os_str()).collect();
        assert!(Cue::try_parse_from(&refs).is_err());
    }

    #[test]
    fn emits_spec_for_docs_and_completions() {
        let kdl = Cue::to_kdl();
        for expected in ["doctor", "models", "config", "cache", "--verbose", "--fix"] {
            assert!(kdl.contains(expected), "spec missing {expected}:\n{kdl}");
        }
    }
}
