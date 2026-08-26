use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use cue_core::{CueError, Result};

const MEDIA_EXTENSIONS: &[&str] = &[
    "aac", "aif", "aiff", "flac", "m4a", "mp3", "ogg", "opus", "wav", "avi", "m2ts", "m4v", "mkv",
    "mov", "mp4", "mpeg", "mpg", "mts", "ts", "webm",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedInput {
    pub source: PathBuf,
    pub output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InputPlan {
    pub inputs: Vec<ResolvedInput>,
    pub is_batch: bool,
}

pub(super) fn resolve_inputs(
    paths: &[PathBuf],
    recursive: bool,
    output: Option<&Path>,
) -> Result<InputPlan> {
    let mut discovered = Vec::new();
    let mut contains_directory = false;

    for path in paths {
        let metadata = fs::symlink_metadata(path).map_err(|err| {
            let summary = if err.kind() == std::io::ErrorKind::NotFound {
                format!("input path {} does not exist", path.display())
            } else {
                format!("could not inspect input path {}", path.display())
            };
            CueError::general(summary).because(err.to_string())
        })?;
        let file_type = metadata.file_type();

        if file_type.is_dir() {
            contains_directory = true;
            let mut files = discover_directory(path, recursive)?;
            if files.is_empty() {
                let mut error = CueError::general(format!(
                    "directory {} contains no recognized media files",
                    path.display()
                ));
                if !recursive && has_real_subdirectory(path)? {
                    error = error.remedy("retry with `--recursive` to search nested directories");
                } else {
                    error = error
                        .remedy("provide a directory containing supported audio or video files");
                }
                return Err(error);
            }
            discovered.append(&mut files);
        } else if file_type.is_file() {
            discovered.push(path.clone());
        } else if file_type.is_symlink() {
            let target = fs::canonicalize(path).map_err(|err| {
                CueError::general(format!(
                    "could not resolve input symlink {}",
                    path.display()
                ))
                .because(err.to_string())
            })?;
            let target_metadata = fs::metadata(&target).map_err(|err| {
                CueError::general(format!(
                    "could not inspect input symlink {}",
                    path.display()
                ))
                .because(err.to_string())
            })?;
            if target_metadata.is_dir() {
                return Err(CueError::general(format!(
                    "input {} is a symlinked directory",
                    path.display()
                ))
                .remedy("pass the real directory path instead"));
            }
            if !target_metadata.is_file() {
                return Err(CueError::general(format!(
                    "input {} is not a supported file or directory",
                    path.display()
                )));
            }
            discovered.push(path.clone());
        } else {
            return Err(CueError::general(format!(
                "input {} is not a regular file or directory",
                path.display()
            )));
        }
    }

    let is_batch = paths.len() > 1 || contains_directory;
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    for source in discovered {
        let canonical = fs::canonicalize(&source).map_err(|err| {
            CueError::general(format!("could not resolve input file {}", source.display()))
                .because(err.to_string())
        })?;
        if seen.insert(canonical) {
            sources.push(source);
        }
    }

    let inputs = sources
        .into_iter()
        .map(|source| {
            let output = output_directory(&source, output, is_batch);
            ResolvedInput { source, output }
        })
        .collect::<Vec<_>>();
    reject_output_collisions(&inputs)?;

    Ok(InputPlan { inputs, is_batch })
}

fn discover_directory(root: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_directory(root, recursive, &mut files)?;
    files.sort_by(|left, right| {
        left.strip_prefix(root)
            .unwrap_or(left)
            .cmp(right.strip_prefix(root).unwrap_or(right))
    });
    Ok(files)
}

fn collect_directory(directory: &Path, recursive: bool, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(directory).map_err(|err| {
        CueError::general(format!("could not read directory {}", directory.display()))
            .because(err.to_string())
    })?;

    for entry in entries {
        let entry = entry.map_err(|err| {
            CueError::general(format!("could not read directory {}", directory.display()))
                .because(err.to_string())
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|err| {
            CueError::general(format!("could not inspect {}", path.display()))
                .because(err.to_string())
        })?;

        if file_type.is_dir() {
            if recursive {
                collect_directory(&path, true, files)?;
            }
        } else if file_type.is_file() {
            if is_media_path(&path) {
                files.push(path);
            }
        } else if file_type.is_symlink() && is_media_path(&path) {
            let target = fs::canonicalize(&path).map_err(|err| {
                CueError::general(format!(
                    "could not resolve media symlink {}",
                    path.display()
                ))
                .because(err.to_string())
            })?;
            let metadata = fs::metadata(&target).map_err(|err| {
                CueError::general(format!(
                    "could not inspect media symlink {}",
                    path.display()
                ))
                .because(err.to_string())
            })?;
            if metadata.is_file() {
                files.push(path);
            }
            // Directory symlinks are deliberately ignored, even recursively.
        }
    }

    Ok(())
}

fn has_real_subdirectory(directory: &Path) -> Result<bool> {
    let entries = fs::read_dir(directory).map_err(|err| {
        CueError::general(format!("could not read directory {}", directory.display()))
            .because(err.to_string())
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            CueError::general(format!("could not read directory {}", directory.display()))
                .because(err.to_string())
        })?;
        if entry
            .file_type()
            .map_err(|err| {
                CueError::general(format!("could not inspect {}", entry.path().display()))
                    .because(err.to_string())
            })?
            .is_dir()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_media_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            MEDIA_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

fn output_directory(source: &Path, root: Option<&Path>, is_batch: bool) -> PathBuf {
    if let Some(root) = root {
        if !is_batch {
            return root.to_owned();
        }
        return root.join(cue_directory_name(source));
    }

    source
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(cue_directory_name(source))
}

fn cue_directory_name(source: &Path) -> String {
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy())
        .unwrap_or_else(|| "output".into());
    format!("{stem}.cue")
}

fn reject_output_collisions(inputs: &[ResolvedInput]) -> Result<()> {
    let mut destinations: HashMap<String, &ResolvedInput> = HashMap::new();
    for input in inputs {
        let folded = input.output.to_string_lossy().to_lowercase();
        if let Some(previous) = destinations.insert(folded, input) {
            return Err(
                CueError::general("multiple inputs resolve to the same output directory")
                    .because(format!(
                        "{} -> {}; {} -> {}",
                        previous.source.display(),
                        previous.output.display(),
                        input.source.display(),
                        input.output.display()
                    ))
                    .remedy(
                        "rename one input or process the files with separate --output directories",
                    ),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"media").unwrap();
    }

    #[test]
    fn discovers_supported_media_in_stable_order() {
        let temp = tempfile::tempdir().unwrap();
        touch(&temp.path().join("b.MP4"));
        touch(&temp.path().join("a.mp3"));
        touch(&temp.path().join("notes.txt"));

        let plan = resolve_inputs(&[temp.path().to_owned()], false, None).unwrap();

        assert!(plan.is_batch);
        assert_eq!(
            plan.inputs
                .iter()
                .map(|input| input.source.file_name().unwrap().to_owned())
                .collect::<Vec<_>>(),
            ["a.mp3", "b.MP4"]
        );
    }

    #[test]
    fn directory_allowlist_is_ascii_case_insensitive_and_complete() {
        let temp = tempfile::tempdir().unwrap();
        for extension in MEDIA_EXTENSIONS {
            touch(&temp.path().join(format!("clip-{extension}.{extension}")));
        }
        touch(&temp.path().join("uppercase.WEBM"));
        touch(&temp.path().join("not-media.json"));

        let plan = resolve_inputs(&[temp.path().to_owned()], false, None).unwrap();

        assert_eq!(plan.inputs.len(), MEDIA_EXTENSIONS.len() + 1);
        assert!(
            plan.inputs
                .iter()
                .any(|input| input.source.ends_with("uppercase.WEBM"))
        );
        assert!(
            !plan
                .inputs
                .iter()
                .any(|input| input.source.ends_with("not-media.json"))
        );
    }

    #[test]
    fn positional_groups_keep_their_order_while_each_directory_is_sorted() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.mp4");
        let directory = temp.path().join("directory");
        let last = temp.path().join("last.mp4");
        touch(&first);
        touch(&directory.join("b.mp4"));
        touch(&directory.join("a.mp4"));
        touch(&last);

        let plan = resolve_inputs(&[first.clone(), directory, last.clone()], false, None).unwrap();

        assert_eq!(
            plan.inputs
                .iter()
                .map(|input| input.source.clone())
                .collect::<Vec<_>>(),
            [
                first,
                temp.path().join("directory/a.mp4"),
                temp.path().join("directory/b.mp4"),
                last
            ]
        );
    }

    #[test]
    fn recursion_is_opt_in_and_sorts_by_relative_path() {
        let temp = tempfile::tempdir().unwrap();
        touch(&temp.path().join("top.wav"));
        touch(&temp.path().join("z/second.mp4"));
        touch(&temp.path().join("a/first.mkv"));

        let shallow = resolve_inputs(&[temp.path().to_owned()], false, None).unwrap();
        assert_eq!(shallow.inputs.len(), 1);

        let recursive = resolve_inputs(&[temp.path().to_owned()], true, None).unwrap();
        assert_eq!(
            recursive
                .inputs
                .iter()
                .map(|input| input.source.strip_prefix(temp.path()).unwrap().to_owned())
                .collect::<Vec<_>>(),
            [
                PathBuf::from("a/first.mkv"),
                PathBuf::from("top.wav"),
                PathBuf::from("z/second.mp4")
            ]
        );
    }

    #[test]
    fn explicit_files_are_not_filtered_by_extension() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("recording.custom");
        touch(&source);

        let plan = resolve_inputs(std::slice::from_ref(&source), false, None).unwrap();

        assert_eq!(plan.inputs[0].source, source);
        assert!(!plan.is_batch);
    }

    #[test]
    fn canonical_paths_are_deduplicated_at_the_first_position() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("clip.mp4");
        touch(&source);

        let plan = resolve_inputs(&[source.clone(), source.clone()], false, None).unwrap();

        assert!(plan.is_batch);
        assert_eq!(plan.inputs.len(), 1);
        assert_eq!(plan.inputs[0].source, source);
    }

    #[test]
    fn output_mapping_preserves_single_file_compatibility_and_isolates_batches() {
        let temp = tempfile::tempdir().unwrap();
        let one = temp.path().join("one.mp4");
        let two = temp.path().join("two.mp4");
        touch(&one);
        touch(&two);
        let output = temp.path().join("out");

        let single = resolve_inputs(std::slice::from_ref(&one), false, Some(&output)).unwrap();
        assert_eq!(single.inputs[0].output, output);

        let batch = resolve_inputs(&[one, two], false, Some(&output)).unwrap();
        assert_eq!(batch.inputs[0].output, output.join("one.cue"));
        assert_eq!(batch.inputs[1].output, output.join("two.cue"));

        std::fs::create_dir_all(output.join("one.cue")).unwrap();
        let existing = resolve_inputs(
            &[temp.path().join("one.mp4"), temp.path().join("two.mp4")],
            false,
            Some(&output),
        )
        .unwrap();
        assert_eq!(existing.inputs[0].output, output.join("one.cue"));
    }

    #[test]
    fn output_collisions_are_rejected_before_processing() {
        let temp = tempfile::tempdir().unwrap();
        let upper = temp.path().join("a/Same.mp4");
        let lower = temp.path().join("b/same.mp3");
        touch(&upper);
        touch(&lower);

        let error = resolve_inputs(&[upper, lower], false, Some(&temp.path().join("out")))
            .unwrap_err()
            .to_string();

        assert!(error.contains("Same.cue"), "{error}");
        assert!(error.contains("same.cue"), "{error}");
        assert!(error.contains("same output directory"), "{error}");
    }

    #[test]
    fn default_output_collisions_are_also_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let video = temp.path().join("same.mp4");
        let audio = temp.path().join("same.wav");
        touch(&video);
        touch(&audio);

        let error = resolve_inputs(&[video, audio], false, None)
            .unwrap_err()
            .to_string();

        assert!(error.contains("same output directory"), "{error}");
        assert!(error.contains("same.cue"), "{error}");
    }

    #[test]
    fn missing_and_empty_directories_are_errors() {
        let temp = tempfile::tempdir().unwrap();
        let missing = resolve_inputs(&[temp.path().join("missing")], false, None)
            .unwrap_err()
            .to_string();
        assert!(missing.contains("does not exist"), "{missing}");

        let nested = temp.path().join("empty/nested");
        std::fs::create_dir_all(&nested).unwrap();
        let empty = resolve_inputs(&[temp.path().join("empty")], false, None)
            .unwrap_err()
            .to_string();
        assert!(empty.contains("no recognized media"), "{empty}");
        assert!(empty.contains("--recursive"), "{empty}");
    }

    #[cfg(unix)]
    #[test]
    fn file_symlinks_are_deduplicated_and_directory_symlinks_are_not_followed() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.mp4");
        let alias = temp.path().join("alias.mp4");
        touch(&source);
        symlink(&source, &alias).unwrap();

        let plan = resolve_inputs(&[alias, source.clone()], false, None).unwrap();
        assert_eq!(plan.inputs.len(), 1);

        let arbitrary_extension = temp.path().join("alias.custom");
        symlink(&source, &arbitrary_extension).unwrap();
        let explicit =
            resolve_inputs(std::slice::from_ref(&arbitrary_extension), false, None).unwrap();
        assert_eq!(explicit.inputs[0].source, arbitrary_extension);

        let real_dir = temp.path().join("real");
        std::fs::create_dir(&real_dir).unwrap();
        let dir_link = temp.path().join("linked");
        symlink(&real_dir, &dir_link).unwrap();
        let error = resolve_inputs(&[dir_link], true, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("symlinked directory"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn broken_allowlisted_symlinks_are_fatal() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let broken = temp.path().join("broken.mp4");
        symlink(temp.path().join("missing.mp4"), &broken).unwrap();

        let error = resolve_inputs(&[broken], false, None)
            .unwrap_err()
            .to_string();

        assert!(error.contains("could not resolve input symlink"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn recursive_discovery_never_descends_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input");
        let external = temp.path().join("external");
        std::fs::create_dir_all(&input).unwrap();
        touch(&external.join("hidden.mp4"));
        symlink(&external, input.join("linked")).unwrap();

        let error = resolve_inputs(&[input], true, None)
            .unwrap_err()
            .to_string();

        assert!(error.contains("no recognized media"), "{error}");
    }
}
