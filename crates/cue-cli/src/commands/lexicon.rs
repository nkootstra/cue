//! Reusable correction lexicon commands.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use cue_core::{CueError, Result};
use fs4::FileExt;

use crate::cli::{LexiconArgs, LexiconCommand};
use crate::render::println_line;

static LEXICON_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(serde::Serialize)]
struct PromotionOutcome {
    schema_version: u8,
    status: &'static str,
    source_receipt_hash: String,
    target_lexicon: String,
    target_lexicon_hash: String,
    find: String,
    replace: String,
}

impl PromotionOutcome {
    fn message(&self) -> String {
        let action = if self.status == "promoted" {
            "Promoted to"
        } else {
            "Already present in"
        };
        format!(
            "{action} {}: {} -> {}",
            self.target_lexicon, self.find, self.replace
        )
    }
}

#[derive(serde::Deserialize)]
struct PromotionReceipt {
    schema_version: u8,
    manifest_hash: Option<String>,
    manifest_path: Option<String>,
    #[serde(default)]
    manifests: Vec<PromotionManifest>,
    source_hashes: PromotionSourceHashes,
    rules: Vec<PromotionRule>,
}

#[derive(serde::Deserialize)]
struct PromotionManifest {
    hash: String,
    path: String,
}

#[derive(serde::Deserialize)]
struct PromotionSourceHashes {
    transcript: String,
    normalized: Option<String>,
}

#[derive(serde::Deserialize)]
struct PromotionRule {
    find: String,
    replace: String,
}

pub fn run(args: LexiconArgs, output_root: Option<&Path>) -> i32 {
    let Some(command) = args.command else {
        println_line("Usage: cue lexicon promote <OUTPUT> --rule <PHRASE> --to <DIRECTORY>");
        return 0;
    };

    let result = match command {
        LexiconCommand::Promote(args) => {
            crate::commands::correct::resolve_output_dir_at(&args.output, output_root)
                .and_then(|output_dir| promote_applied_rule(&output_dir, &args.rule, &args.to))
                .and_then(|outcome| {
                    if args.json {
                        serde_json::to_string_pretty(&outcome).map_err(|error| {
                            CueError::general("could not serialize promotion attestation")
                                .because(error.to_string())
                        })
                    } else {
                        Ok(outcome.message())
                    }
                })
        }
    };

    match result {
        Ok(outcome) => {
            println_line(&outcome);
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn promote_applied_rule(
    output_dir: &Path,
    find: &str,
    target_dir: &Path,
) -> Result<PromotionOutcome> {
    if !target_dir.is_dir() {
        return Err(CueError::general(format!(
            "lexicon target {} is not an existing directory",
            target_dir.display()
        ))
        .remedy("create the directory or choose an existing project, course, or folder scope"));
    }

    let receipt_path = output_dir.join("corrections.applied.json");
    let receipt_bytes = std::fs::read(&receipt_path).map_err(|error| {
        CueError::general(format!("could not read {}", receipt_path.display()))
            .because(error.to_string())
    })?;
    let receipt: PromotionReceipt = serde_json::from_slice(&receipt_bytes).map_err(|error| {
        CueError::general(format!("could not parse {}", receipt_path.display()))
            .because(error.to_string())
    })?;
    let source_receipt_hash = cue_cache::bytes_hash(&receipt_bytes);
    if !matches!(receipt.schema_version, 1 | 2) {
        return Err(CueError::general(format!(
            "corrections receipt uses unsupported schema version {}",
            receipt.schema_version
        ))
        .remedy("re-run `cue correct` with this version of cue before promoting"));
    }
    let manifest_paths = validate_receipt_provenance(output_dir, &receipt)?;
    let rule = receipt
        .rules
        .iter()
        .find(|rule| rule.find.eq_ignore_ascii_case(find))
        .ok_or_else(|| {
            CueError::general(format!(
                "correction rule {find:?} is not in the applied receipt"
            ))
            .remedy("run `cue correct`, then choose a rule listed in its receipt")
        })?;
    if !crate::corrections::verified_rule_applies(
        output_dir,
        &manifest_paths,
        &rule.find,
        &rule.replace,
    )? {
        return Err(CueError::general(format!(
            "correction rule {:?} did not match any rendered artifact",
            rule.find
        ))
        .remedy("promote only a correction that was verified in this output"));
    }

    let lexicon_path = target_dir.join("corrections.md");
    let _lock = acquire_lexicon_lock(target_dir)?;
    let mut content = match std::fs::read_to_string(&lexicon_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(
                CueError::general(format!("could not read {}", lexicon_path.display()))
                    .because(error.to_string()),
            );
        }
    };
    let existing = cue_core::correct::parse_manifest(&content)?;
    if let Some(current) = existing
        .iter()
        .find(|current| current.old.eq_ignore_ascii_case(&rule.find))
    {
        if current.new == rule.replace {
            return Ok(PromotionOutcome {
                schema_version: 1,
                status: "already-present",
                source_receipt_hash,
                target_lexicon: lexicon_path.to_string_lossy().into_owned(),
                target_lexicon_hash: cue_cache::bytes_hash(content.as_bytes()),
                find: rule.find.clone(),
                replace: rule.replace.clone(),
            });
        }
        return Err(CueError::general(format!(
            "{} already maps {:?} to {:?}",
            lexicon_path.display(),
            current.old,
            current.new
        ))
        .remedy("review the conflict and edit the target lexicon explicitly"));
    }
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&format!("{} -> {}\n", rule.find, rule.replace));
    write_atomic(&lexicon_path, content.as_bytes())?;

    Ok(PromotionOutcome {
        schema_version: 1,
        status: "promoted",
        source_receipt_hash,
        target_lexicon: lexicon_path.to_string_lossy().into_owned(),
        target_lexicon_hash: cue_cache::bytes_hash(content.as_bytes()),
        find: rule.find.clone(),
        replace: rule.replace.clone(),
    })
}

fn validate_receipt_provenance(
    output_dir: &Path,
    receipt: &PromotionReceipt,
) -> Result<Vec<std::path::PathBuf>> {
    let manifests = if receipt.schema_version == 1 {
        vec![(
            receipt.manifest_path.as_deref().ok_or_else(stale_receipt)?,
            receipt.manifest_hash.as_deref().ok_or_else(stale_receipt)?,
        )]
    } else {
        if receipt.manifests.is_empty() {
            return Err(stale_receipt());
        }
        receipt
            .manifests
            .iter()
            .map(|manifest| (manifest.path.as_str(), manifest.hash.as_str()))
            .collect()
    };
    let mut manifest_paths = Vec::with_capacity(manifests.len());
    for (path, expected_hash) in manifests {
        let path = Path::new(path);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            output_dir.join(path)
        };
        validate_hash(&path, expected_hash)?;
        manifest_paths.push(path);
    }
    validate_hash(
        &output_dir.join("transcript.json"),
        &receipt.source_hashes.transcript,
    )?;
    let normalized_path = output_dir.join("normalized.json");
    match &receipt.source_hashes.normalized {
        Some(expected_hash) => validate_hash(&normalized_path, expected_hash)?,
        None if normalized_path.exists() => return Err(stale_receipt()),
        None => {}
    }
    Ok(manifest_paths)
}

fn validate_hash(path: &Path, expected_hash: &str) -> Result<()> {
    let bytes = std::fs::read(path).map_err(|_| stale_receipt())?;
    if cue_cache::bytes_hash(&bytes) != expected_hash {
        return Err(stale_receipt());
    }
    Ok(())
}

fn stale_receipt() -> CueError {
    CueError::general("corrections receipt no longer matches its source files")
        .remedy("re-run `cue correct` before promoting this rule")
}

fn acquire_lexicon_lock(target_dir: &Path) -> Result<std::fs::File> {
    let lock_path = target_dir.join(".corrections.md.lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| {
            CueError::general(format!("could not open {}", lock_path.display()))
                .because(error.to_string())
        })?;
    FileExt::lock(&file).map_err(|error| {
        CueError::general(format!("could not lock {}", lock_path.display()))
            .because(error.to_string())
    })?;
    Ok(file)
}

fn write_atomic(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        CueError::general(format!(
            "could not determine the parent of {}",
            path.display()
        ))
    })?;
    loop {
        let sequence = LEXICON_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".corrections.md.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(CueError::general(format!(
                    "could not create a temporary lexicon in {}",
                    parent.display()
                ))
                .because(error.to_string()));
            }
        };
        if let Err(error) = file.write_all(content).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&temp_path);
            return Err(
                CueError::general(format!("could not write {}", path.display()))
                    .because(error.to_string()),
            );
        }
        drop(file);
        match std::fs::rename(&temp_path, path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let _ = std::fs::remove_file(&temp_path);
                return Err(
                    CueError::general(format!("could not publish {}", path.display()))
                        .because(error.to_string()),
                );
            }
        }
    }
}
