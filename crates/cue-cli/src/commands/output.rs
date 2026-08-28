use std::path::{Path, PathBuf};

use cue_core::{CueError, Result};

pub(super) const WORKSPACE_FILE: &str = "cue.workspace.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct WorkspaceDescriptor {
    schema_version: u8,
    source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutputLayout {
    pub workspace: PathBuf,
    pub published_base: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OwnedPublishedOutput {
    path: PathBuf,
    digest: String,
}

pub(super) fn source_layout(source: &Path, root: Option<&Path>) -> Result<OutputLayout> {
    let stem = source
        .file_stem()
        .unwrap_or_else(|| std::ffi::OsStr::new("output"));
    if let Some(root) = root {
        return Ok(OutputLayout {
            workspace: root.join(".cue").join(stem),
            published_base: root.join(stem),
        });
    }

    let parent = source.parent().unwrap_or_else(|| Path::new(""));
    let legacy = parent.join(format!("{}.cue", stem.to_string_lossy()));
    let hidden = parent.join(".cue").join(stem);
    let workspace = select_workspace(source, &legacy, &hidden)?;
    Ok(OutputLayout {
        workspace,
        published_base: parent.join(stem),
    })
}

pub(super) fn existing_source_layout(source: &Path, root: Option<&Path>) -> Result<OutputLayout> {
    if let Some(root) = root {
        let layout = source_layout(source, Some(root))?;
        if layout.workspace.is_dir() {
            return Ok(layout);
        }
        let source = std::fs::canonicalize(source).map_err(|error| {
            CueError::general(format!("could not resolve source {}", source.display()))
                .because(error.to_string())
        })?;
        let mut matches = Vec::new();
        find_source_workspaces(&root.join(".cue"), &source, &mut matches)?;
        match matches.as_slice() {
            [workspace] => return Ok(workspace_layout(workspace)),
            [] => {
                return Err(CueError::general(format!(
                    "no cue workspace beneath {} refers to {}",
                    root.display(),
                    source.display()
                ))
                .remedy("pass the output root used when the media was processed"));
            }
            _ => {
                return Err(CueError::general(format!(
                    "multiple cue workspaces beneath {} refer to {}",
                    root.display(),
                    source.display()
                ))
                .remedy("pass the intended workspace directory explicitly"));
            }
        }
    }

    let layout = source_layout(source, None)?;
    if layout.workspace.is_dir() {
        return Ok(layout);
    }
    Err(CueError::general(format!(
        "no cue output directory found at {}",
        source.display()
    ))
    .remedy("pass a cue workspace directory, or media with `.cue/<stem>/` or `<stem>.cue/` state"))
}

pub(crate) fn workspace_layout(workspace: &Path) -> OutputLayout {
    let published_base = if let Some(cue_root) = workspace
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == ".cue"))
    {
        let relative = workspace.strip_prefix(cue_root).unwrap_or(workspace);
        cue_root
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(relative)
    } else {
        let name = workspace
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("output")
            .strip_suffix(".cue")
            .unwrap_or("output");
        workspace
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(name)
    };
    OutputLayout {
        workspace: workspace.to_owned(),
        published_base,
    }
}

pub(super) fn write_workspace_descriptor(workspace: &Path, source: &Path) -> Result<()> {
    let descriptor = WorkspaceDescriptor {
        schema_version: 1,
        source: crate::run_contract::tracked_reference(workspace, source)?,
    };
    let mut bytes = serde_json::to_vec_pretty(&descriptor).map_err(|error| {
        CueError::general("could not serialize workspace descriptor").because(error.to_string())
    })?;
    bytes.push(b'\n');
    crate::run_contract::write_atomic(&workspace.join(WORKSPACE_FILE), &bytes)
        .map_err(|error| error.at_stage(cue_core::PipelineStage::Render))
}

fn find_source_workspaces(root: &Path, source: &Path, matches: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(CueError::general(format!(
                "could not search cue workspaces beneath {}",
                root.display()
            ))
            .because(error.to_string()));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            CueError::general(format!(
                "could not search cue workspaces beneath {}",
                root.display()
            ))
            .because(error.to_string())
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CueError::general(format!("could not inspect {}", path.display()))
                .because(error.to_string())
        })?;
        if !file_type.is_dir() {
            continue;
        }
        let descriptor_path = path.join(WORKSPACE_FILE);
        if descriptor_path.is_file()
            && let Ok(bytes) = std::fs::read(&descriptor_path)
            && let Ok(descriptor) = serde_json::from_slice::<WorkspaceDescriptor>(&bytes)
            && descriptor.schema_version == 1
            && std::fs::canonicalize(path.join(descriptor.source))
                .ok()
                .as_deref()
                == Some(source)
        {
            matches.push(path.clone());
        }
        find_source_workspaces(&path, source, matches)?;
    }
    Ok(())
}

fn select_workspace(source: &Path, legacy: &Path, hidden: &Path) -> Result<PathBuf> {
    match (legacy.is_dir(), hidden.is_dir()) {
        (true, true) => Err(CueError::general(format!(
            "multiple cue workspaces exist for {}",
            source.display()
        ))
        .because(format!("{}; {}", legacy.display(), hidden.display()))
        .remedy("pass the intended workspace directory explicitly")),
        (true, false) => Ok(legacy.to_owned()),
        (false, true) => Ok(hidden.to_owned()),
        (false, false) => Ok(hidden.to_owned()),
    }
}

pub(super) fn publish_subtitles(
    layout: &OutputLayout,
    formats: impl IntoIterator<Item = cue_core::config::SubtitleFormat>,
    skip_missing: bool,
) -> Result<Vec<PathBuf>> {
    if let Some(parent) = layout.published_base.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CueError::new(
                cue_core::PipelineStage::Render,
                format!("could not create output directory {}", parent.display()),
            )
            .because(error.to_string())
        })?;
    }

    let mut published = Vec::new();
    for format in formats {
        let source = layout
            .workspace
            .join(format!("subtitles.{}", format.extension()));
        if skip_missing && !source.is_file() {
            continue;
        }
        let content = std::fs::read(&source).map_err(|error| {
            CueError::new(
                cue_core::PipelineStage::Render,
                format!("could not read rendered subtitle {}", source.display()),
            )
            .because(error.to_string())
        })?;
        let destination = subtitle_path(&layout.published_base, format.extension());
        crate::run_contract::write_atomic(&destination, &content)
            .map_err(|error| error.at_stage(cue_core::PipelineStage::Render))?;
        published.push(destination);
    }
    Ok(published)
}

pub(super) fn preflight_subtitles(
    layout: &OutputLayout,
    formats: impl IntoIterator<Item = cue_core::config::SubtitleFormat>,
    skip_missing_workspace_format: bool,
) -> Result<()> {
    for format in formats {
        let workspace_subtitle = layout
            .workspace
            .join(format!("subtitles.{}", format.extension()));
        if skip_missing_workspace_format && !workspace_subtitle.is_file() {
            continue;
        }
        let destination = subtitle_path(&layout.published_base, format.extension());
        let metadata = match std::fs::symlink_metadata(&destination) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(CueError::general(format!(
                    "could not inspect subtitle destination {}",
                    destination.display()
                ))
                .because(error.to_string()));
            }
        };
        if !metadata.file_type().is_file() {
            return Err(CueError::general(format!(
                "subtitle destination {} is not a regular file",
                destination.display()
            )));
        }

        let destination_hash = cue_cache::file_hash(&destination)?;
        let matches_workspace = workspace_subtitle.is_file()
            && cue_cache::file_hash(&workspace_subtitle)? == destination_hash;
        let recorded_by_receipt =
            crate::run_contract::RunReceipt::read_for_verification(&layout.workspace)
                .ok()
                .is_some_and(|receipt| {
                    let destination = std::fs::canonicalize(&destination).ok();
                    receipt.published_outputs.iter().any(|published| {
                        std::fs::canonicalize(layout.workspace.join(&published.path)).ok()
                            == destination
                            && published.digest.value == destination_hash
                    })
                });
        if !matches_workspace && !recorded_by_receipt {
            return Err(CueError::general(format!(
                "subtitle destination {} already exists and is not owned by cue",
                destination.display()
            ))
            .remedy("move the existing subtitle or choose a different --output root"));
        }
    }
    Ok(())
}

pub(super) fn owned_published_outputs(layout: &OutputLayout) -> Vec<OwnedPublishedOutput> {
    let Ok(receipt) = crate::run_contract::RunReceipt::read_for_verification(&layout.workspace)
    else {
        return Vec::new();
    };
    receipt
        .published_outputs
        .into_iter()
        .filter_map(|published| {
            let path = layout.workspace.join(published.path);
            crate::run_contract::is_regular_file(&path, false)
                .ok()
                .filter(|is_regular| *is_regular)?;
            let digest = cue_cache::file_hash(&path).ok()?;
            (digest == published.digest.value).then_some(OwnedPublishedOutput { path, digest })
        })
        .collect()
}

pub(super) fn remove_stale_published_outputs(
    previous: &[OwnedPublishedOutput],
    current: &[PathBuf],
) -> Result<()> {
    for output in previous {
        if current.iter().any(|path| path == &output.path) {
            continue;
        }
        let is_regular =
            crate::run_contract::is_regular_file(&output.path, false).map_err(|error| {
                CueError::general(format!(
                    "could not inspect stale subtitle {}",
                    output.path.display()
                ))
                .because(error.to_string())
            })?;
        if !is_regular || cue_cache::file_hash(&output.path)? != output.digest {
            continue;
        }
        std::fs::remove_file(&output.path).map_err(|error| {
            CueError::general(format!(
                "could not remove stale subtitle {}",
                output.path.display()
            ))
            .because(error.to_string())
        })?;
    }
    Ok(())
}

fn subtitle_path(base: &Path, extension: &str) -> PathBuf {
    let mut path = base.as_os_str().to_owned();
    path.push(".");
    path.push(extension);
    PathBuf::from(path)
}

pub(crate) fn published_subtitle_path(layout: &OutputLayout, extension: &str) -> PathBuf {
    subtitle_path(&layout.published_base, extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_output_root_never_falls_back_to_an_adjacent_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("lesson.mp4");
        std::fs::write(&source, b"media").unwrap();
        std::fs::create_dir_all(temp.path().join(".cue/lesson")).unwrap();
        let wrong_root = temp.path().join("elsewhere");

        let error = existing_source_layout(&source, Some(&wrong_root))
            .unwrap_err()
            .to_string();

        assert!(error.contains("no cue workspace beneath"), "{error}");
        assert!(error.contains(&wrong_root.display().to_string()), "{error}");
    }

    #[test]
    fn explicit_output_root_finds_a_nested_workspace_by_descriptor() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("course/module/lesson.mp4");
        let root = temp.path().join("out");
        let workspace = root.join(".cue/module/lesson");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(&source, b"media").unwrap();
        write_workspace_descriptor(&workspace, &source).unwrap();

        let layout = existing_source_layout(&source, Some(&root)).unwrap();

        assert_eq!(layout.workspace, workspace);
        assert_eq!(layout.published_base, root.join("module/lesson"));
    }

    #[test]
    fn subtitle_paths_preserve_dots_in_media_stems() {
        assert_eq!(
            subtitle_path(Path::new("4. Setting up Rust"), "srt"),
            PathBuf::from("4. Setting up Rust.srt")
        );
    }

    #[test]
    fn preflight_refuses_unowned_visible_subtitles() {
        let temp = tempfile::tempdir().unwrap();
        let layout = OutputLayout {
            workspace: temp.path().join(".cue/lesson"),
            published_base: temp.path().join("lesson"),
        };
        std::fs::write(temp.path().join("lesson.srt"), "user subtitle\n").unwrap();

        let error = preflight_subtitles(&layout, [cue_core::config::SubtitleFormat::Srt], false)
            .unwrap_err()
            .to_string();

        assert!(error.contains("not owned by cue"), "{error}");
    }

    #[test]
    fn preflight_accepts_a_sidecar_matching_the_workspace_copy() {
        let temp = tempfile::tempdir().unwrap();
        let layout = OutputLayout {
            workspace: temp.path().join(".cue/lesson"),
            published_base: temp.path().join("lesson"),
        };
        std::fs::create_dir_all(&layout.workspace).unwrap();
        std::fs::write(layout.workspace.join("subtitles.srt"), "cue subtitle\n").unwrap();
        std::fs::write(temp.path().join("lesson.srt"), "cue subtitle\n").unwrap();

        preflight_subtitles(&layout, [cue_core::config::SubtitleFormat::Srt], false).unwrap();
    }

    #[test]
    fn stale_cleanup_removes_only_unchanged_cue_owned_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let stale = temp.path().join("lesson.vtt");
        let changed = temp.path().join("lesson.srt");
        std::fs::write(&stale, "WEBVTT\n").unwrap();
        std::fs::write(&changed, "original\n").unwrap();
        let previous = vec![
            OwnedPublishedOutput {
                path: stale.clone(),
                digest: cue_cache::file_hash(&stale).unwrap(),
            },
            OwnedPublishedOutput {
                path: changed.clone(),
                digest: cue_cache::file_hash(&changed).unwrap(),
            },
        ];
        std::fs::write(&changed, "user changed\n").unwrap();

        remove_stale_published_outputs(&previous, &[]).unwrap();

        assert!(!stale.exists());
        assert_eq!(std::fs::read_to_string(changed).unwrap(), "user changed\n");
    }

    #[test]
    fn stale_cleanup_keeps_current_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("lesson.srt");
        std::fs::write(&current, "subtitle\n").unwrap();
        let previous = vec![OwnedPublishedOutput {
            path: current.clone(),
            digest: cue_cache::file_hash(&current).unwrap(),
        }];

        remove_stale_published_outputs(&previous, std::slice::from_ref(&current)).unwrap();

        assert!(current.is_file());
    }
}
