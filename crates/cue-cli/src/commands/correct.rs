//! `cue correct` — apply transcript corrections from a manifest.
//!
//! Rewrites the spoken-text artifacts of a cue output directory
//! (transcript.txt, transcript.clean.txt, subtitles.srt/.vtt) using the
//! deterministic engine in cue-core. The raw transcript.json is never
//! touched.

use std::path::{Path, PathBuf};

use cue_core::{CueError, Result};

use crate::cli::CorrectArgs;
use crate::render::println_line;

/// Artifacts that mirror the spoken words and may be corrected.
const CORRECTABLE_FILES: [&str; 4] = [
    "transcript.txt",
    "transcript.clean.txt",
    "subtitles.srt",
    "subtitles.vtt",
];

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

/// Locate the manifest: explicit path, else corrections.md in the output
/// dir, else in its parent.
fn find_manifest(output_dir: &Path, explicit: Option<&str>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        let p = PathBuf::from(path);
        if !p.exists() {
            return Err(CueError::general(format!(
                "corrections manifest {} does not exist",
                p.display()
            )));
        }
        return Ok(p);
    }
    let in_dir = output_dir.join("corrections.md");
    if in_dir.exists() {
        return Ok(in_dir);
    }
    let parent = output_dir.parent().map(|p| p.join("corrections.md"));
    if let Some(parent) = parent
        && parent.exists()
    {
        return Ok(parent);
    }
    Err(CueError::general("no corrections manifest found").remedy(
        "create a `corrections.md` in the output directory or its \
             parent, or pass --corrections <file>",
    ))
}

pub fn run(args: CorrectArgs) -> i32 {
    match run_inner(&args) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn run_inner(args: &CorrectArgs) -> Result<()> {
    let output_dir = resolve_output_dir(&args.output)?;
    let manifest = find_manifest(&output_dir, args.corrections.as_deref())?;

    let text = std::fs::read_to_string(&manifest).map_err(|e| {
        CueError::general(format!(
            "could not read corrections manifest {}",
            manifest.display()
        ))
        .because(e.to_string())
    })?;
    let rules = cue_core::correct::parse_manifest(&text)?;
    if rules.is_empty() {
        return Err(CueError::general("corrections manifest has no rules")
            .remedy("add lines of the form `phrase to find -> replacement`"));
    }

    // Apply to each correctable artifact that exists.
    let mut changed_any = false;
    for name in CORRECTABLE_FILES {
        let path = output_dir.join(name);
        if !path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&path).map_err(|e| {
            CueError::general(format!("could not read {}", path.display())).because(e.to_string())
        })?;
        let (corrected, replacements) = cue_core::correct::apply_counted(&content, &rules);
        if replacements == 0 {
            continue;
        }
        changed_any = true;
        println_line(&format!(
            "  {}: {replacements} replacement(s)",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        if !args.dry_run {
            std::fs::write(&path, corrected).map_err(|e| {
                CueError::general(format!("could not write {}", path.display()))
                    .because(e.to_string())
            })?;
        }
    }

    if !changed_any {
        println_line("  no changes: no correctable files matched the manifest");
        return Ok(());
    }
    if args.dry_run {
        println_line("\nDry run — nothing written. Re-run without --dry-run to apply.");
    } else {
        println_line("\nApplied. transcript.json and analysis outputs were left untouched.");
    }
    Ok(())
}
