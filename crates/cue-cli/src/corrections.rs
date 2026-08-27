//! Durable correction rendering shared by `cue correct` and media processing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cue_core::{CueError, Result};

const RECEIPT_FILE: &str = "corrections.applied.json";
const CORRECTABLE_FILES: [&str; 4] = [
    "transcript.txt",
    "transcript.clean.txt",
    "subtitles.srt",
    "subtitles.vtt",
];

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ManifestSource {
    Explicit,
    OutputDirectory,
    ParentDirectory,
}

#[derive(Clone)]
pub(crate) struct PreparedManifest {
    bytes: Vec<u8>,
    rules: Vec<cue_core::correct::Correction>,
    source: ManifestSource,
}

pub(crate) enum CorrectionPlan {
    None,
    Apply(PreparedManifest),
}

impl CorrectionPlan {
    /// Resolve and validate correction plans for a complete processing batch.
    /// An explicit manifest is shared, so it is read and parsed exactly once.
    pub(crate) fn prepare_batch<'a>(
        output_dirs: impl IntoIterator<Item = &'a Path>,
        explicit: Option<&Path>,
    ) -> Result<HashMap<PathBuf, Self>> {
        let output_dirs = output_dirs.into_iter().collect::<Vec<_>>();
        if let Some(path) = explicit {
            if !path.exists() {
                return Err(CueError::general(format!(
                    "corrections manifest {} does not exist",
                    path.display()
                )));
            }
            let manifest = PreparedManifest::read(path, ManifestSource::Explicit)?;
            return Ok(output_dirs
                .into_iter()
                .map(|output_dir| (output_dir.to_path_buf(), Self::Apply(manifest.clone())))
                .collect());
        }

        output_dirs
            .into_iter()
            .map(|output_dir| {
                Self::prepare(output_dir, None)
                    .map(|correction| (output_dir.to_path_buf(), correction))
            })
            .collect()
    }

    /// Resolve and validate the manifest for one output directory. Absence is
    /// a valid plan for ordinary media processing.
    pub(crate) fn prepare(output_dir: &Path, explicit: Option<&Path>) -> Result<Self> {
        let Some((path, source)) = find_manifest(output_dir, explicit)? else {
            return Ok(Self::None);
        };
        Ok(Self::Apply(PreparedManifest::read(&path, source)?))
    }

    /// Resolve a manifest for the explicit correction command, where absence
    /// means the requested operation cannot be performed.
    pub(crate) fn require(output_dir: &Path, explicit: Option<&Path>) -> Result<Self> {
        match Self::prepare(output_dir, explicit)? {
            Self::None => Err(CueError::general("no corrections manifest found").remedy(
                "create a `corrections.md` in the output directory or its parent, or pass \
                 --corrections <file>",
            )),
            plan => Ok(plan),
        }
    }

    pub(crate) fn render(
        &self,
        output_dir: &Path,
        config: &cue_core::Config,
        scope: CorrectionScope,
        dry_run: bool,
    ) -> Result<Option<RenderOutcome>> {
        match self {
            Self::None => {
                if !dry_run {
                    if scope == CorrectionScope::TranscriptOnly {
                        clear_non_transcript_artifacts(output_dir)?;
                    }
                    clear_receipt(output_dir)?;
                }
                Ok(None)
            }
            Self::Apply(manifest) => {
                render_output(output_dir, manifest, config, scope, dry_run).map(Some)
            }
        }
    }
}

fn find_manifest(
    output_dir: &Path,
    explicit: Option<&Path>,
) -> Result<Option<(PathBuf, ManifestSource)>> {
    if let Some(path) = explicit {
        if !path.exists() {
            return Err(CueError::general(format!(
                "corrections manifest {} does not exist",
                path.display()
            )));
        }
        return Ok(Some((path.to_path_buf(), ManifestSource::Explicit)));
    }

    let in_output = output_dir.join("corrections.md");
    if in_output.exists() {
        return Ok(Some((in_output, ManifestSource::OutputDirectory)));
    }

    if let Some(in_parent) = output_dir.parent().map(|path| path.join("corrections.md"))
        && in_parent.exists()
    {
        return Ok(Some((in_parent, ManifestSource::ParentDirectory)));
    }

    Ok(None)
}

impl PreparedManifest {
    /// Read and validate a manifest without modifying its target output.
    /// Batch processing can prepare every manifest before media work begins.
    pub(crate) fn read(path: &Path, source: ManifestSource) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| {
            CueError::general(format!(
                "could not read corrections manifest {}",
                path.display()
            ))
            .because(e.to_string())
        })?;
        let text = std::str::from_utf8(&bytes).map_err(|e| {
            CueError::general(format!(
                "corrections manifest {} is not valid UTF-8",
                path.display()
            ))
            .because(e.to_string())
        })?;
        let rules = cue_core::correct::parse_manifest(text)?;
        if rules.is_empty() {
            return Err(CueError::general("corrections manifest has no rules")
                .remedy("add lines of the form `phrase to find -> replacement`"));
        }
        Ok(Self {
            bytes,
            rules,
            source,
        })
    }
}

pub(crate) struct RenderOutcome {
    pub(crate) replacements: Vec<(&'static str, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CorrectionScope {
    Full,
    TranscriptOnly,
}

impl RenderOutcome {
    pub(crate) fn changed_any(&self) -> bool {
        self.replacements.iter().any(|(_, count)| *count > 0)
    }
}

#[derive(serde::Serialize)]
struct Receipt<'a> {
    schema_version: u8,
    manifest_hash: String,
    manifest_source: ManifestSource,
    source_hashes: SourceHashes,
    rules: Vec<AppliedRule<'a>>,
}

#[derive(serde::Serialize)]
struct SourceHashes {
    transcript: String,
    normalized: Option<String>,
}

#[derive(serde::Serialize)]
struct AppliedRule<'a> {
    find: &'a str,
    replace: &'a str,
    applications: Vec<RuleApplication>,
}

#[derive(serde::Serialize)]
struct RuleApplication {
    artifact: &'static str,
    replacements: usize,
}

struct PreparedArtifact {
    name: &'static str,
    path: PathBuf,
    content: String,
    counts: Vec<usize>,
}

/// Rebuild correctable artifacts from canonical JSON and apply a prepared
/// manifest. Canonical and analysis artifacts are read-only inputs.
pub(crate) fn render_output(
    output_dir: &Path,
    manifest: &PreparedManifest,
    config: &cue_core::Config,
    scope: CorrectionScope,
    dry_run: bool,
) -> Result<RenderOutcome> {
    let transcript_path = output_dir.join("transcript.json");
    let transcript_bytes = read_file(&transcript_path)?;
    let transcript: cue_core::Transcript = parse_json(&transcript_path, &transcript_bytes)?;
    transcript.validate()?;

    let normalized_path = output_dir.join("normalized.json");
    let normalized = if scope == CorrectionScope::Full && normalized_path.exists() {
        let bytes = read_file(&normalized_path)?;
        let value = parse_json::<cue_core::NormalizedTranscript>(&normalized_path, &bytes)?;
        Some((value, bytes))
    } else {
        None
    };

    let policy = cue_subtitles::SubtitlePolicy {
        max_lines: config.subtitles.max_lines,
        max_chars_per_line: config.subtitles.max_chars_per_line,
        max_duration_ms: config.subtitles.max_duration_ms,
    };
    let subtitle_cues = cue_subtitles::build_cues(&transcript, &policy)?;

    // Build and validate the complete render before mutating any output.
    let mut artifacts = Vec::new();
    for name in CORRECTABLE_FILES {
        let path = output_dir.join(name);
        let configured_subtitle = match name {
            "subtitles.srt" => config
                .subtitles
                .formats
                .contains(&cue_core::config::SubtitleFormat::Srt),
            "subtitles.vtt" => config
                .subtitles
                .formats
                .contains(&cue_core::config::SubtitleFormat::Vtt),
            _ => false,
        };
        let should_render = match name {
            "transcript.txt" => true,
            "transcript.clean.txt" => scope == CorrectionScope::Full && normalized.is_some(),
            "subtitles.srt" | "subtitles.vtt" => {
                scope == CorrectionScope::Full && (path.exists() || configured_subtitle)
            }
            _ => false,
        };
        if !should_render {
            continue;
        }

        let raw = match name {
            "transcript.txt" => transcript.plain_text(),
            "transcript.clean.txt" => normalized
                .as_ref()
                .map(|(value, _)| value.plain_text())
                .unwrap_or_default(),
            "subtitles.srt" => cue_subtitles::render_srt(&subtitle_cues),
            "subtitles.vtt" => cue_subtitles::render_vtt(&subtitle_cues),
            _ => unreachable!("all correctable artifact names are covered"),
        };
        let (content, counts) = cue_core::correct::apply_with_counts(&raw, &manifest.rules);
        artifacts.push(PreparedArtifact {
            name,
            path,
            content,
            counts,
        });
    }

    let receipt = Receipt {
        schema_version: 1,
        manifest_hash: cue_cache::bytes_hash(&manifest.bytes),
        manifest_source: manifest.source,
        source_hashes: SourceHashes {
            transcript: cue_cache::bytes_hash(&transcript_bytes),
            normalized: normalized
                .as_ref()
                .map(|(_, bytes)| cue_cache::bytes_hash(bytes)),
        },
        rules: manifest
            .rules
            .iter()
            .enumerate()
            .map(|(rule_index, rule)| AppliedRule {
                find: rule.old.as_str(),
                replace: rule.new.as_str(),
                applications: artifacts
                    .iter()
                    .map(|artifact| RuleApplication {
                        artifact: artifact.name,
                        replacements: artifact.counts[rule_index],
                    })
                    .collect(),
            })
            .collect(),
    };
    let mut receipt_json = serde_json::to_string_pretty(&receipt).map_err(|e| {
        CueError::general("could not serialize corrections receipt").because(e.to_string())
    })?;
    receipt_json.push('\n');

    let outcome = RenderOutcome {
        replacements: artifacts
            .iter()
            .map(|artifact| (artifact.name, artifact.counts.iter().sum()))
            .collect(),
    };
    if dry_run {
        return Ok(outcome);
    }

    let receipt_path = output_dir.join(RECEIPT_FILE);
    remove_if_present(&receipt_path, "stale corrections receipt")?;
    if scope == CorrectionScope::TranscriptOnly {
        clear_non_transcript_artifacts(output_dir)?;
    }
    if scope == CorrectionScope::Full && normalized.is_none() {
        remove_if_present(
            &output_dir.join("transcript.clean.txt"),
            "stale cleaned transcript",
        )?;
    }
    for artifact in artifacts {
        std::fs::write(&artifact.path, artifact.content).map_err(|e| {
            CueError::general(format!("could not write {}", artifact.path.display()))
                .because(e.to_string())
        })?;
    }
    // The receipt is the commit marker for a complete corrected render.
    std::fs::write(&receipt_path, receipt_json).map_err(|e| {
        CueError::general(format!("could not write {}", receipt_path.display()))
            .because(e.to_string())
    })?;

    Ok(outcome)
}

/// Remove the marker for a corrected render when normal processing has no
/// authoritative manifest. Missing receipts are already clean state.
pub(crate) fn clear_receipt(output_dir: &Path) -> Result<()> {
    remove_if_present(&output_dir.join(RECEIPT_FILE), "stale corrections receipt")
}

fn clear_non_transcript_artifacts(output_dir: &Path) -> Result<()> {
    for name in [
        "normalized.json",
        "transcript.clean.txt",
        "subtitles.srt",
        "subtitles.vtt",
        "analysis.json",
        "summary.md",
        "description.md",
    ] {
        remove_if_present(
            &output_dir.join(name),
            "artifact outside transcript-only scope",
        )?;
    }
    Ok(())
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| {
        CueError::general(format!("could not read {}", path.display())).because(e.to_string())
    })
}

fn parse_json<T: serde::de::DeserializeOwned>(path: &Path, bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| {
        CueError::general(format!("could not parse {}", path.display())).because(e.to_string())
    })
}

fn remove_if_present(path: &Path, description: &str) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(CueError::general(format!(
            "could not remove {description} {}",
            path.display()
        ))
        .because(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_transcript(output: &Path) {
        std::fs::write(
            output.join("transcript.json"),
            r#"{
  "schema_version": 1,
  "language": "en",
  "duration_ms": 1000,
  "words": [
    {"text":"open","start_ms":0,"end_ms":300,"confidence":0.9,"speaker":null},
    {"text":"telemetry.","start_ms":310,"end_ms":1000,"confidence":0.9,"speaker":null}
  ],
  "segments": [
    {"start_ms":0,"end_ms":1000,"text":"open telemetry.","word_start":0,"word_end":2}
  ]
}"#,
        )
        .unwrap();
    }

    #[test]
    fn transcript_only_render_leaves_only_transcript_artifacts_and_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("lesson.cue");
        std::fs::create_dir(&output).unwrap();
        write_transcript(&output);
        std::fs::write(output.join("transcript.txt"), "old render\n").unwrap();
        for stale in [
            "normalized.json",
            "transcript.clean.txt",
            "subtitles.srt",
            "subtitles.vtt",
            "analysis.json",
            "summary.md",
            "description.md",
        ] {
            std::fs::write(output.join(stale), "STALE\n").unwrap();
        }
        let manifest = temp.path().join("corrections.md");
        std::fs::write(&manifest, "open telemetry -> OpenTelemetry\n").unwrap();

        CorrectionPlan::prepare(&output, Some(&manifest))
            .unwrap()
            .render(
                &output,
                &cue_core::Config::default(),
                CorrectionScope::TranscriptOnly,
                false,
            )
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(output.join("transcript.txt")).unwrap(),
            "OpenTelemetry.\n"
        );
        assert!(output.join("corrections.applied.json").exists());
        for absent in [
            "normalized.json",
            "transcript.clean.txt",
            "subtitles.srt",
            "subtitles.vtt",
            "analysis.json",
            "summary.md",
            "description.md",
        ] {
            assert!(!output.join(absent).exists(), "{absent} should be absent");
        }
    }

    #[test]
    fn transcript_only_render_without_manifest_clears_stale_correction_state() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("lesson.cue");
        std::fs::create_dir(&output).unwrap();
        write_transcript(&output);
        std::fs::write(output.join("transcript.txt"), "open telemetry.\n").unwrap();
        std::fs::write(output.join("corrections.applied.json"), "STALE\n").unwrap();
        std::fs::write(output.join("subtitles.srt"), "STALE\n").unwrap();

        CorrectionPlan::prepare(&output, None)
            .unwrap()
            .render(
                &output,
                &cue_core::Config::default(),
                CorrectionScope::TranscriptOnly,
                false,
            )
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(output.join("transcript.txt")).unwrap(),
            "open telemetry.\n"
        );
        assert!(!output.join("corrections.applied.json").exists());
        assert!(!output.join("subtitles.srt").exists());
    }

    #[test]
    fn full_render_without_manifest_clears_only_the_stale_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("lesson.cue");
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("transcript.txt"), "open telemetry.\n").unwrap();
        std::fs::write(output.join("analysis.json"), "ANALYSIS\n").unwrap();
        std::fs::write(output.join("corrections.applied.json"), "STALE\n").unwrap();

        CorrectionPlan::prepare(&output, None)
            .unwrap()
            .render(
                &output,
                &cue_core::Config::default(),
                CorrectionScope::Full,
                false,
            )
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(output.join("transcript.txt")).unwrap(),
            "open telemetry.\n"
        );
        assert_eq!(
            std::fs::read_to_string(output.join("analysis.json")).unwrap(),
            "ANALYSIS\n"
        );
        assert!(!output.join("corrections.applied.json").exists());
    }
}
