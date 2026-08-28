//! Durable correction rendering shared by `cue correct` and media processing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cue_core::config::SubtitleFormat;
use cue_core::{CueError, Result};

const RECEIPT_FILE: &str = "corrections.applied.json";

#[derive(Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ManifestSource {
    Explicit,
    OutputDirectory,
    ParentDirectory,
}

#[derive(Clone)]
pub(crate) struct PreparedManifest {
    contents: Arc<ManifestContents>,
    source: ManifestSource,
    pub(crate) path: PathBuf,
}

struct ManifestContents {
    bytes: Vec<u8>,
    rules: Vec<cue_core::correct::Correction>,
}

#[derive(Clone)]
pub(crate) struct PreparedCorrections {
    pub(crate) manifests: Vec<PreparedManifest>,
    pub(crate) rules: Vec<ResolvedRule>,
    pub(crate) conflicts: Vec<RuleConflict>,
}

#[derive(Clone)]
pub(crate) struct ResolvedRule {
    pub(crate) correction: cue_core::correct::Correction,
    pub(crate) source_manifest: usize,
}

#[derive(Clone)]
pub(crate) struct RuleConflict {
    pub(crate) find: String,
    pub(crate) winner: String,
    pub(crate) shadowed: String,
    pub(crate) winner_manifest: usize,
    pub(crate) shadowed_manifest: usize,
}

pub(crate) enum CorrectionPlan {
    None,
    Apply(Arc<PreparedCorrections>),
}

impl CorrectionPlan {
    /// Resolve and validate correction plans for a complete processing batch.
    /// An explicit manifest is shared, so it is read and parsed exactly once.
    pub(crate) fn prepare_batch<'a>(
        output_dirs: impl IntoIterator<Item = &'a Path>,
        explicit: Option<&Path>,
    ) -> Result<HashMap<PathBuf, Self>> {
        if let Some(path) = explicit {
            if !path.exists() {
                return Err(CueError::general(format!(
                    "corrections manifest {} does not exist",
                    path.display()
                )));
            }
            let manifest = PreparedManifest::read_required(path, ManifestSource::Explicit)?;
            let corrections = Arc::new(PreparedCorrections::from_manifests(vec![manifest]));
            return Ok(output_dirs
                .into_iter()
                .map(|output_dir| {
                    (
                        output_dir.to_path_buf(),
                        Self::Apply(Arc::clone(&corrections)),
                    )
                })
                .collect());
        }

        let mut manifest_cache = HashMap::<PathBuf, Arc<ManifestContents>>::new();
        let mut plan_cache =
            HashMap::<Vec<(PathBuf, ManifestSource)>, Arc<PreparedCorrections>>::new();
        output_dirs
            .into_iter()
            .map(|output_dir| {
                let manifests = find_manifests(output_dir, None)?;
                if manifests.is_empty() {
                    return Ok((output_dir.to_path_buf(), Self::None));
                }
                let manifests = manifests
                    .into_iter()
                    .map(|(path, source)| (std::fs::canonicalize(&path).unwrap_or(path), source))
                    .collect::<Vec<_>>();
                if let Some(corrections) = plan_cache.get(&manifests) {
                    return Ok((
                        output_dir.to_path_buf(),
                        Self::Apply(Arc::clone(corrections)),
                    ));
                }
                let prepared = manifests
                    .iter()
                    .map(|(path, source)| {
                        let contents = match manifest_cache.get(path) {
                            Some(contents) => Arc::clone(contents),
                            None => {
                                let contents = PreparedManifest::read_contents(path)?;
                                manifest_cache.insert(path.clone(), Arc::clone(&contents));
                                contents
                            }
                        };
                        Ok(PreparedManifest::if_contributing(
                            contents,
                            path.clone(),
                            *source,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>();
                if prepared.is_empty() {
                    return Ok((output_dir.to_path_buf(), Self::None));
                }
                let corrections = Arc::new(PreparedCorrections::from_manifests(prepared));
                plan_cache.insert(manifests, Arc::clone(&corrections));
                Ok((output_dir.to_path_buf(), Self::Apply(corrections)))
            })
            .collect()
    }

    /// Resolve and validate the manifest for one output directory. Absence is
    /// a valid plan for ordinary media processing.
    pub(crate) fn prepare(output_dir: &Path, explicit: Option<&Path>) -> Result<Self> {
        let manifests = find_manifests(output_dir, explicit)?;
        if manifests.is_empty() {
            return Ok(Self::None);
        }
        if explicit.is_some() {
            let (path, source) = &manifests[0];
            let manifest = PreparedManifest::read_required(path, *source)?;
            return Ok(Self::Apply(Arc::new(PreparedCorrections::from_manifests(
                vec![manifest],
            ))));
        }
        Ok(match PreparedCorrections::read_all(manifests)? {
            Some(corrections) => Self::Apply(Arc::new(corrections)),
            None => Self::None,
        })
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
    ) -> Result<RenderOutcome> {
        let result = match self {
            Self::None => {
                if !dry_run {
                    if scope == CorrectionScope::TranscriptOnly {
                        clear_non_transcript_artifacts(output_dir)?;
                    }
                    clear_receipt(output_dir)?;
                }
                Ok(RenderOutcome::default())
            }
            Self::Apply(manifest) => render_output(output_dir, manifest, config, scope, dry_run),
        };
        result.map_err(|error| error.at_stage(cue_core::PipelineStage::Render))
    }

    /// Invalidate the commit marker before normal processing mutates any
    /// canonical or derived output.
    pub(crate) fn invalidate_receipt(&self, output_dir: &Path) -> Result<()> {
        clear_receipt(output_dir).map_err(|error| error.at_stage(cue_core::PipelineStage::Render))
    }

    /// Apply this plan to an in-memory transcript using the same word-aware
    /// transformation used for corrected subtitle rendering.
    pub(crate) fn apply_to_transcript(
        &self,
        transcript: &mut cue_core::Transcript,
    ) -> Vec<cue_subtitles::CueSource> {
        let mut sources = (0..transcript.words.len())
            .map(|word_start| cue_subtitles::CueSource {
                word_start,
                word_end: word_start + 1,
            })
            .collect::<Vec<_>>();
        if let Self::Apply(corrections) = self {
            corrections.apply_to_transcript_tracking_sources(transcript, Some(&mut sources));
        }
        sources
    }
}

impl PreparedCorrections {
    fn apply_to_transcript(&self, transcript: &mut cue_core::Transcript) -> Vec<usize> {
        self.apply_to_transcript_tracking_sources(transcript, None)
    }

    fn apply_to_transcript_tracking_sources(
        &self,
        transcript: &mut cue_core::Transcript,
        sources: Option<&mut [cue_subtitles::CueSource]>,
    ) -> Vec<usize> {
        let rules = self
            .rules
            .iter()
            .map(|rule| rule.correction.clone())
            .collect::<Vec<_>>();
        let mut counts = vec![0; rules.len()];
        apply_rules_across_words(&mut transcript.words, &rules, &mut counts, sources);
        counts
    }
}

fn find_manifests(
    output_dir: &Path,
    explicit: Option<&Path>,
) -> Result<Vec<(PathBuf, ManifestSource)>> {
    if let Some(path) = explicit {
        if !path.exists() {
            return Err(CueError::general(format!(
                "corrections manifest {} does not exist",
                path.display()
            )));
        }
        return Ok(vec![(path.to_path_buf(), ManifestSource::Explicit)]);
    }

    let mut manifests = Vec::new();
    let in_output = output_dir.join("corrections.md");
    if in_output.exists() {
        manifests.push((in_output, ManifestSource::OutputDirectory));
    }

    let current_dir = std::env::current_dir().map_err(|error| {
        CueError::general("could not determine the current directory").because(error.to_string())
    })?;
    let current_dir = std::fs::canonicalize(&current_dir).unwrap_or(current_dir);
    let absolute_output = if output_dir.is_absolute() {
        output_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                CueError::general("could not determine the current directory")
                    .because(error.to_string())
            })?
            .join(output_dir)
    };
    let absolute_output = std::fs::canonicalize(&absolute_output).unwrap_or_else(|_| {
        absolute_output
            .parent()
            .and_then(|parent| std::fs::canonicalize(parent).ok())
            .and_then(|parent| absolute_output.file_name().map(|name| parent.join(name)))
            .unwrap_or(absolute_output)
    });
    if absolute_output == current_dir {
        return Ok(manifests);
    }
    let inside_current_dir = absolute_output.starts_with(&current_dir);
    for ancestor in absolute_output.ancestors().skip(1) {
        let path = ancestor.join("corrections.md");
        if path.exists() {
            manifests.push((path, ManifestSource::ParentDirectory));
        }
        if !inside_current_dir || ancestor == current_dir {
            break;
        }
    }

    manifests.reverse();
    Ok(manifests)
}

impl PreparedManifest {
    /// Read and validate a manifest without modifying its target output.
    /// Batch processing can prepare every manifest before media work begins.
    fn read_required(path: &Path, source: ManifestSource) -> Result<Self> {
        let contents = Self::read_contents(path)?;
        if contents.rules.is_empty() {
            return Err(CueError::general(format!(
                "corrections manifest has no rules: {}",
                path.display()
            ))
            .remedy("add lines of the form `phrase to find -> replacement`"));
        }
        Ok(Self {
            contents,
            source,
            path: path.to_path_buf(),
        })
    }

    fn read_contents(path: &Path) -> Result<Arc<ManifestContents>> {
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
        Ok(Arc::new(ManifestContents { bytes, rules }))
    }

    fn if_contributing(
        contents: Arc<ManifestContents>,
        path: PathBuf,
        source: ManifestSource,
    ) -> Option<Self> {
        (!contents.rules.is_empty()).then_some(Self {
            contents,
            source,
            path,
        })
    }
}

impl PreparedCorrections {
    fn read_all(manifests: Vec<(PathBuf, ManifestSource)>) -> Result<Option<Self>> {
        let prepared = manifests
            .into_iter()
            .map(|(path, source)| {
                let contents = PreparedManifest::read_contents(&path)?;
                Ok(PreparedManifest::if_contributing(contents, path, source))
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        Ok((!prepared.is_empty()).then(|| Self::from_manifests(prepared)))
    }

    fn from_manifests(manifests: Vec<PreparedManifest>) -> Self {
        let mut conflicts = Vec::new();
        let mut winners = HashMap::<String, (usize, usize, ResolvedRule)>::new();

        for (manifest_index, manifest) in manifests.iter().enumerate() {
            for (rule_index, rule) in manifest.contents.rules.iter().enumerate() {
                let key = rule.old.to_ascii_lowercase();
                if let Some((_, _, shadowed)) = winners.get(&key)
                    && shadowed.correction.new != rule.new
                {
                    conflicts.push(RuleConflict {
                        find: rule.old.clone(),
                        winner: rule.new.clone(),
                        shadowed: shadowed.correction.new.clone(),
                        winner_manifest: manifest_index,
                        shadowed_manifest: shadowed.source_manifest,
                    });
                }
                winners.insert(
                    key,
                    (
                        manifest_index,
                        rule_index,
                        ResolvedRule {
                            correction: rule.clone(),
                            source_manifest: manifest_index,
                        },
                    ),
                );
            }
        }

        conflicts.retain_mut(|conflict| {
            if let Some((_, _, winner)) = winners.get(&conflict.find.to_ascii_lowercase()) {
                conflict.winner.clone_from(&winner.correction.new);
                conflict.winner_manifest = winner.source_manifest;
            }
            conflict.winner != conflict.shadowed
        });

        let mut rules = Vec::with_capacity(winners.len());
        for (manifest_index, manifest) in manifests.iter().enumerate() {
            for (rule_index, rule) in manifest.contents.rules.iter().enumerate() {
                let key = rule.old.to_ascii_lowercase();
                if winners
                    .get(&key)
                    .is_some_and(|(winner_manifest, winner_rule, _)| {
                        (*winner_manifest, *winner_rule) == (manifest_index, rule_index)
                    })
                    && let Some((_, _, winner)) = winners.remove(&key)
                {
                    rules.push(winner);
                }
            }
        }

        Self {
            manifests,
            rules,
            conflicts,
        }
    }
}

#[derive(Default)]
pub(crate) struct RenderOutcome {
    pub(crate) replacements: Vec<(&'static str, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CorrectionScope {
    Full,
    TranscriptOnly,
}

impl RenderOutcome {
    pub(crate) fn has_replacements(&self) -> bool {
        self.replacements.iter().any(|(_, count)| *count > 0)
    }
}

#[derive(serde::Serialize)]
struct Receipt<'a> {
    schema_version: u8,
    manifests: Vec<ReceiptManifest>,
    source_hashes: SourceHashes,
    rules: Vec<AppliedRule<'a>>,
}

#[derive(serde::Serialize)]
struct ReceiptManifest {
    hash: String,
    path: String,
    source: ManifestSource,
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
    source_manifest: usize,
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
fn render_output(
    output_dir: &Path,
    manifest: &PreparedCorrections,
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
    let rules = manifest
        .rules
        .iter()
        .map(|rule| rule.correction.clone())
        .collect::<Vec<_>>();

    // Build and validate the complete render before mutating any output.
    let mut artifacts = vec![prepare_artifact(
        output_dir,
        "transcript.txt",
        transcript.plain_text(),
        &rules,
    )];
    if scope == CorrectionScope::Full
        && let Some((clean, _)) = &normalized
    {
        artifacts.push(prepare_artifact(
            output_dir,
            "transcript.clean.txt",
            clean.plain_text(),
            &rules,
        ));
    }

    let subtitle_formats = [SubtitleFormat::Srt, SubtitleFormat::Vtt]
        .into_iter()
        .filter(|format| {
            let path = output_dir.join(format!("subtitles.{}", format.extension()));
            scope == CorrectionScope::Full
                && (path.exists() || config.subtitles.formats.contains(format))
        })
        .collect::<Vec<_>>();
    if !subtitle_formats.is_empty() {
        let policy = cue_subtitles::SubtitlePolicy {
            max_lines: config.subtitles.max_lines,
            max_chars_per_line: config.subtitles.max_chars_per_line,
            max_duration_ms: config.subtitles.max_duration_ms,
        };
        let mut corrected_transcript = transcript.clone();
        let subtitle_counts = manifest.apply_to_transcript(&mut corrected_transcript);
        let cues = cue_subtitles::build_cues(&corrected_transcript, &policy)?;
        for format in subtitle_formats {
            let (name, content) = match format {
                SubtitleFormat::Srt => ("subtitles.srt", cue_subtitles::render_srt(&cues)),
                SubtitleFormat::Vtt => ("subtitles.vtt", cue_subtitles::render_vtt(&cues)),
            };
            artifacts.push(PreparedArtifact {
                name,
                path: output_dir.join(name),
                content,
                counts: subtitle_counts.clone(),
            });
        }
    }

    let receipt = Receipt {
        schema_version: 2,
        manifests: manifest
            .manifests
            .iter()
            .map(|source| ReceiptManifest {
                hash: cue_cache::bytes_hash(&source.contents.bytes),
                path: manifest_reference(output_dir, &source.path),
                source: source.source,
            })
            .collect(),
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
                find: rule.correction.old.as_str(),
                replace: rule.correction.new.as_str(),
                source_manifest: rule.source_manifest,
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
fn clear_receipt(output_dir: &Path) -> Result<()> {
    remove_if_present(&output_dir.join(RECEIPT_FILE), "stale corrections receipt")
}

fn prepare_artifact(
    output_dir: &Path,
    name: &'static str,
    raw: String,
    rules: &[cue_core::correct::Correction],
) -> PreparedArtifact {
    let (content, counts) = cue_core::correct::apply_with_counts(&raw, rules);
    PreparedArtifact {
        name,
        path: output_dir.join(name),
        content,
        counts,
    }
}

fn apply_rules_across_words(
    words: &mut [cue_core::Word],
    rules: &[cue_core::correct::Correction],
    counts: &mut [usize],
    mut sources: Option<&mut [cue_subtitles::CueSource]>,
) {
    for (rule_index, rule) in rules.iter().enumerate() {
        let mut start = 0;
        while start < words.len() {
            let mut matched = None;
            for end in start..words.len() {
                let candidate = words[start..=end]
                    .iter()
                    .map(|word| word.text.trim())
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                if candidate.contains(&rule.old) {
                    matched = Some((end, candidate));
                    break;
                }
            }
            let Some((end, candidate)) = matched else {
                start += 1;
                continue;
            };
            let applications = candidate.matches(&rule.old).count();
            let replacement = candidate.replace(&rule.old, &rule.new);
            let end_ms = words[end].end_ms;
            words[start].text = replacement;
            words[start].end_ms = end_ms;
            if let Some(sources) = sources.as_deref_mut() {
                sources[start].word_start = sources[start].word_start.min(sources[end].word_start);
                sources[start].word_end = sources[start].word_end.max(sources[end].word_end);
            }
            for word in &mut words[start + 1..=end] {
                word.text.clear();
            }
            counts[rule_index] += applications;
            start = end + 1;
        }
    }
}

pub(crate) fn manifest_reference(output_dir: &Path, manifest_path: &Path) -> String {
    let output_dir = std::fs::canonicalize(output_dir).unwrap_or_else(|_| output_dir.to_path_buf());
    let manifest_path =
        std::fs::canonicalize(manifest_path).unwrap_or_else(|_| manifest_path.to_path_buf());
    if let Ok(path) = manifest_path.strip_prefix(&output_dir) {
        return path.to_string_lossy().into_owned();
    }
    if let Some(parent) = manifest_path.parent()
        && let Ok(relative_output) = output_dir.strip_prefix(parent)
    {
        let levels = relative_output.components().count();
        return format!(
            "{}{}",
            "../".repeat(levels),
            manifest_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
    }
    manifest_path.to_string_lossy().into_owned()
}

pub(crate) fn verified_rule_applies(
    output_dir: &Path,
    manifest_paths: &[PathBuf],
    find: &str,
    replace: &str,
) -> Result<bool> {
    let manifests = manifest_paths
        .iter()
        .map(|path| PreparedManifest::read_required(path, ManifestSource::Explicit))
        .collect::<Result<Vec<_>>>()?;
    let prepared = PreparedCorrections::from_manifests(manifests);
    let rules = prepared
        .rules
        .iter()
        .map(|rule| rule.correction.clone())
        .collect::<Vec<_>>();
    let Some(index) = prepared.rules.iter().position(|rule| {
        rule.correction.old.eq_ignore_ascii_case(find) && rule.correction.new == replace
    }) else {
        return Ok(false);
    };

    let transcript_path = output_dir.join("transcript.json");
    let transcript_bytes = read_file(&transcript_path)?;
    let transcript: cue_core::Transcript = parse_json(&transcript_path, &transcript_bytes)?;
    transcript.validate()?;
    if cue_core::correct::apply_with_counts(&transcript.plain_text(), &rules).1[index] > 0 {
        return Ok(true);
    }

    let normalized_path = output_dir.join("normalized.json");
    if normalized_path.exists() {
        let normalized_bytes = read_file(&normalized_path)?;
        let normalized: cue_core::NormalizedTranscript =
            parse_json(&normalized_path, &normalized_bytes)?;
        if cue_core::correct::apply_with_counts(&normalized.plain_text(), &rules).1[index] > 0 {
            return Ok(true);
        }
    }
    Ok(false)
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

    #[test]
    fn correction_render_failures_are_attributed_to_render() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("lesson.cue");
        std::fs::create_dir(&output).unwrap();
        write_transcript(&output);
        std::fs::create_dir(output.join("transcript.txt")).unwrap();
        let manifest = temp.path().join("corrections.md");
        std::fs::write(&manifest, "open telemetry -> OpenTelemetry\n").unwrap();

        let result = CorrectionPlan::prepare(&output, Some(&manifest))
            .unwrap()
            .render(
                &output,
                &cue_core::Config::default(),
                CorrectionScope::Full,
                false,
            );
        let error = match result {
            Ok(_) => panic!("render unexpectedly succeeded"),
            Err(error) => error,
        };

        assert_eq!(error.stage(), Some(cue_core::PipelineStage::Render));
    }

    #[test]
    fn receipt_invalidation_failures_are_attributed_to_render() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("lesson.cue");
        std::fs::create_dir(&output).unwrap();
        std::fs::create_dir(output.join("corrections.applied.json")).unwrap();
        let plan = CorrectionPlan::prepare(&output, None).unwrap();

        let error = plan.invalidate_receipt(&output).unwrap_err();

        assert_eq!(error.stage(), Some(cue_core::PipelineStage::Render));
    }

    #[test]
    fn batch_plans_filter_empty_discovered_manifests() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("course/lesson.cue");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(
            temp.path().join("course/corrections.md"),
            "# no course-wide corrections yet\n",
        )
        .unwrap();
        let contributing = output.join("corrections.md");
        std::fs::write(&contributing, "open telemetry -> OpenTelemetry\n").unwrap();

        let plans = CorrectionPlan::prepare_batch([output.as_path()], None).unwrap();
        let CorrectionPlan::Apply(plan) = plans.get(&output).unwrap() else {
            panic!("expected a correction plan");
        };

        assert_eq!(plan.manifests.len(), 1);
        assert_eq!(plan.manifests[0].path, contributing.canonicalize().unwrap());
    }

    #[test]
    fn all_empty_discovered_batch_plan_is_none() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("course/lesson.cue");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(
            temp.path().join("course/corrections.md"),
            "# no course-wide corrections yet\n",
        )
        .unwrap();

        let plans = CorrectionPlan::prepare_batch([output.as_path()], None).unwrap();

        assert!(matches!(plans.get(&output), Some(CorrectionPlan::None)));
    }

    #[test]
    fn conflicts_report_the_final_effective_winner() {
        let temp = tempfile::tempdir().unwrap();
        let manifests = ["x -> 1\n", "x -> 2\n", "x -> 3\n"]
            .into_iter()
            .enumerate()
            .map(|(index, contents)| {
                let path = temp.path().join(format!("scope-{index}.md"));
                std::fs::write(&path, contents).unwrap();
                PreparedManifest::read_required(&path, ManifestSource::Explicit).unwrap()
            })
            .collect();

        let prepared = PreparedCorrections::from_manifests(manifests);

        assert_eq!(prepared.conflicts.len(), 2);
        assert!(
            prepared
                .conflicts
                .iter()
                .all(|conflict| conflict.winner == "3" && conflict.winner_manifest == 2)
        );
        assert_eq!(prepared.conflicts[0].shadowed, "1");
        assert_eq!(prepared.conflicts[0].shadowed_manifest, 0);
        assert_eq!(prepared.conflicts[1].shadowed, "2");
        assert_eq!(prepared.conflicts[1].shadowed_manifest, 1);
    }

    #[test]
    fn conflicts_omit_shadowed_values_equal_to_the_final_winner() {
        let temp = tempfile::tempdir().unwrap();
        let manifests = ["x -> 1\n", "x -> 2\n", "x -> 1\n"]
            .into_iter()
            .enumerate()
            .map(|(index, contents)| {
                let path = temp.path().join(format!("scope-{index}.md"));
                std::fs::write(&path, contents).unwrap();
                PreparedManifest::read_required(&path, ManifestSource::Explicit).unwrap()
            })
            .collect();

        let prepared = PreparedCorrections::from_manifests(manifests);

        assert_eq!(prepared.conflicts.len(), 1);
        let conflict = &prepared.conflicts[0];
        assert_eq!(conflict.winner, "1");
        assert_eq!(conflict.winner_manifest, 2);
        assert_eq!(conflict.shadowed, "2");
        assert_eq!(conflict.shadowed_manifest, 1);
    }
}
