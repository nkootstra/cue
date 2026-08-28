//! `cue correct` — rebuild transcript derivatives with a correction manifest.

use std::path::{Path, PathBuf};

use cue_core::Result;

use crate::cli::CorrectArgs;
use crate::corrections::{CorrectionPlan, CorrectionScope};
use crate::render::println_line;

/// Resolve a durable workspace directly or from a media file. New workspaces
/// live under `.cue/<stem>/`; visible `<stem>.cue/` workspaces remain valid.
pub(crate) fn resolve_output_dir_at(path: &Path, root: Option<&Path>) -> Result<PathBuf> {
    resolve_output_layout(path, root).map(|layout| layout.workspace)
}

fn resolve_output_layout(
    path: &Path,
    root: Option<&Path>,
) -> Result<crate::commands::output::OutputLayout> {
    if path.is_dir() {
        return Ok(crate::commands::output::workspace_layout(path));
    }
    crate::commands::output::existing_source_layout(path, root)
}

pub fn run(
    args: CorrectArgs,
    corrections: Option<&Path>,
    output_root: Option<&Path>,
    config: &cue_core::Config,
) -> i32 {
    match run_inner(&args, corrections, output_root, config) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn run_inner(
    args: &CorrectArgs,
    corrections: Option<&Path>,
    output_root: Option<&Path>,
    config: &cue_core::Config,
) -> Result<()> {
    let layout = resolve_output_layout(args.output.as_path(), output_root)?;
    let output_dir = layout.workspace.clone();
    let subtitle_formats = [
        cue_core::config::SubtitleFormat::Srt,
        cue_core::config::SubtitleFormat::Vtt,
    ];
    if !args.dry_run {
        crate::commands::output::preflight_subtitles(&layout, subtitle_formats, true)?;
    }
    let plan = CorrectionPlan::require(&output_dir, corrections)?;
    let _output_lock = if args.dry_run {
        None
    } else {
        Some(crate::run_contract::OutputLock::acquire(&output_dir)?)
    };
    if _output_lock.is_some() {
        crate::run_contract::invalidate(&output_dir)?;
    }
    let outcome = plan.render(&output_dir, config, CorrectionScope::Full, args.dry_run)?;
    if !args.dry_run {
        crate::commands::output::publish_subtitles(&layout, subtitle_formats, true)?;
    }

    for (artifact, replacements) in &outcome.replacements {
        if *replacements > 0 {
            println_line(&format!("  {artifact}: {replacements} replacement(s)"));
        }
    }
    if !outcome.has_replacements() {
        println_line("  no replacements; derived artifacts rebuilt from canonical sources");
    }
    if args.dry_run {
        println_line("\nDry run — nothing written. Re-run without --dry-run to apply.");
    } else {
        println_line("\nApplied. transcript.json and analysis outputs were left untouched.");
    }
    Ok(())
}
