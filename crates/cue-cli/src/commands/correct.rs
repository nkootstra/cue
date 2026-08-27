//! `cue correct` — rebuild transcript derivatives with a correction manifest.

use std::path::{Path, PathBuf};

use cue_core::{CueError, Result};

use crate::cli::CorrectArgs;
use crate::corrections::{CorrectionPlan, CorrectionScope};
use crate::render::println_line;

/// Resolve the output directory: a `.cue/` dir directly, or the sibling
/// `.cue/` of a media file.
fn resolve_output_dir(output: &str) -> Result<PathBuf> {
    let path = Path::new(output);
    if path.is_dir() {
        return Ok(path.to_path_buf());
    }
    if let Some(stem) = path.file_stem() {
        let sibling = path.with_file_name(format!("{}.cue", stem.to_string_lossy()));
        if sibling.is_dir() {
            return Ok(sibling);
        }
    }
    Err(CueError::general(format!(
        "no cue output directory found at {}",
        path.display()
    ))
    .remedy(
        "pass a `<file>.cue/` directory, or a media file whose sibling \
         `.cue/` directory already exists",
    ))
}

pub fn run(args: CorrectArgs, corrections: Option<&Path>, config: &cue_core::Config) -> i32 {
    match run_inner(&args, corrections, config) {
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
    config: &cue_core::Config,
) -> Result<()> {
    let output_dir = resolve_output_dir(&args.output)?;
    let plan = CorrectionPlan::require(&output_dir, corrections)?;
    let outcome = plan
        .render(&output_dir, config, CorrectionScope::Full, args.dry_run)?
        .expect("required correction plans always render an outcome");

    for (artifact, replacements) in &outcome.replacements {
        if *replacements > 0 {
            println_line(&format!("  {artifact}: {replacements} replacement(s)"));
        }
    }
    if !outcome.changed_any() {
        println_line("  no changes: no correctable files matched the manifest");
    }
    if args.dry_run {
        println_line("\nDry run — nothing written. Re-run without --dry-run to apply.");
    } else {
        println_line("\nApplied. transcript.json and analysis outputs were left untouched.");
    }
    Ok(())
}
