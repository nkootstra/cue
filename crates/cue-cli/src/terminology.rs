//! Local, data-driven terminology evidence for transcript review.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use cue_core::{Result, Transcript};

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Evidence {
    pub path: String,
    pub line: usize,
    pub source_kind: &'static str,
    pub authoritative: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub id: String,
    pub observed: String,
    pub proposed: String,
    pub word_index: usize,
    pub confidence: Option<f32>,
    pub score: f32,
    pub evidence: Vec<Evidence>,
}

pub(crate) fn find_candidates(
    output_dir: &Path,
    transcript: &Transcript,
    confidence_below: f32,
    context_root: Option<&Path>,
) -> Result<Vec<Candidate>> {
    let source = source_for_workspace(output_dir);
    let media_root = source
        .as_deref()
        .and_then(Path::parent)
        .unwrap_or_else(|| output_dir.parent().unwrap_or_else(|| Path::new(".")));
    let mut terms = HashMap::<String, Term>::new();
    let mut budget = DiscoveryBudget::default();
    collect_terms(media_root, media_root, &mut terms, false, &mut budget)?;
    if let Some(root) = context_root {
        let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_owned());
        collect_terms(&root, &root, &mut terms, true, &mut budget)?;
    }
    collect_sibling_transcripts(media_root, output_dir, &mut terms, &mut budget)?;

    let mut candidates = Vec::new();
    for (index, word) in transcript.words.iter().enumerate() {
        let code_like = word.text.chars().any(|c| c == '.' || c == '_' || c == '/');
        let eligible = word.confidence.is_none_or(|confidence| {
            confidence < confidence_below || (code_like && confidence < confidence_below.max(0.85))
        });
        if !eligible {
            continue;
        }
        let observed = word.text.trim_matches(|c: char| c.is_ascii_punctuation());
        if normalize(observed).len() < 4 {
            continue;
        }
        let mut best: Option<(&Term, f32)> = None;
        for term in terms.values() {
            let score = if code_like {
                term.score_against(observed)
            } else {
                similarity(observed, &term.normalized)
            };
            // Code-like tokens have a much smaller, more structured search
            // space.  This catches `.tomo` -> `toml` while ordinary prose
            // still uses the stricter confidence-gated path.
            let minimum = if code_like { 0.70 } else { 0.82 };
            if score >= minimum
                && best.as_ref().is_none_or(|(current_term, current_score)| {
                    score > *current_score
                        || (score == *current_score
                            && (term.evidence.len() > current_term.evidence.len()
                                || (term.evidence.len() == current_term.evidence.len()
                                    && (term.normalized.as_str(), term.display.as_str())
                                        < (
                                            current_term.normalized.as_str(),
                                            current_term.display.as_str(),
                                        ))))
                })
            {
                best = Some((term, score));
            }
        }
        let Some((term, score)) = best else {
            continue;
        };
        let repeated_non_authoritative = term
            .evidence
            .iter()
            .filter(|evidence| !evidence.authoritative)
            .count()
            >= 2;
        if normalize(observed) == term.normalized
            || term.evidence.is_empty()
            || (!term.evidence.iter().any(|evidence| evidence.authoritative)
                && !repeated_non_authoritative)
        {
            continue;
        }
        let (candidate_index, observed_text) =
            phrase_context(transcript, index, &term.display, &word.text);
        candidates.push(Candidate {
            id: format!("term-{candidate_index}"),
            observed: observed_text,
            proposed: term.display.clone(),
            word_index: index,
            confidence: word.confidence,
            score,
            evidence: term.evidence.clone(),
        });
    }
    candidates.sort_by_key(|candidate| candidate.word_index);
    candidates.dedup_by(|left, right| left.word_index == right.word_index);
    Ok(candidates)
}

#[derive(Debug, Clone)]
struct Term {
    display: String,
    normalized: String,
    evidence: Vec<Evidence>,
}

impl Term {
    fn score_against(&self, observed: &str) -> f32 {
        let whole = similarity(observed, &self.normalized);
        let parts = self
            .display
            .split(|character: char| !character.is_ascii_alphanumeric())
            .filter(|part| !part.is_empty())
            .map(|part| similarity(observed, part))
            .fold(0.0, f32::max);
        whole.max(parts)
    }
}

const MAX_DISCOVERY_FILES: usize = 512;
const MAX_DISCOVERY_BYTES: usize = 16 * 1_000_000;
const MAX_DISCOVERY_TERMS: usize = 20_000;
const MAX_DISCOVERY_EVIDENCE: usize = 50_000;

#[derive(Debug, Default)]
struct DiscoveryBudget {
    visited_files: HashSet<PathBuf>,
    bytes: usize,
    terms: usize,
    evidence: usize,
}

impl DiscoveryBudget {
    fn visit_file(&mut self, path: &Path, bytes: usize) -> bool {
        let identity = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        if self.visited_files.len() >= MAX_DISCOVERY_FILES
            || self.bytes.saturating_add(bytes) > MAX_DISCOVERY_BYTES
            || !self.visited_files.insert(identity)
        {
            return false;
        }
        self.bytes += bytes;
        true
    }

    fn reserve_term(&mut self) -> bool {
        if self.terms >= MAX_DISCOVERY_TERMS {
            return false;
        }
        self.terms += 1;
        true
    }

    fn reserve_evidence(&mut self) -> bool {
        if self.evidence >= MAX_DISCOVERY_EVIDENCE {
            return false;
        }
        self.evidence += 1;
        true
    }

    fn exhausted(&self) -> bool {
        self.visited_files.len() >= MAX_DISCOVERY_FILES
            || self.bytes >= MAX_DISCOVERY_BYTES
            || self.terms >= MAX_DISCOVERY_TERMS
            || self.evidence >= MAX_DISCOVERY_EVIDENCE
    }
}

fn collect_terms(
    root: &Path,
    path: &Path,
    terms: &mut HashMap<String, Term>,
    explicit: bool,
    budget: &mut DiscoveryBudget,
) -> Result<()> {
    if budget.exhausted() {
        return Ok(());
    }
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.is_dir() {
        let depth = path
            .strip_prefix(root)
            .map(|p| p.components().count())
            .unwrap_or(99);
        if depth > 4
            || (path != root
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.')))
        {
            return Ok(());
        }
        for entry in fs::read_dir(path).map_err(|e| {
            cue_core::CueError::general(format!(
                "could not read terminology directory {}",
                path.display()
            ))
            .because(e.to_string())
        })? {
            collect_terms(
                root,
                &entry
                    .map_err(|e| {
                        cue_core::CueError::general("could not inspect terminology entry")
                            .because(e.to_string())
                    })?
                    .path(),
                terms,
                explicit,
                budget,
            )?;
            if budget.exhausted() {
                break;
            }
        }
        return Ok(());
    }
    if !metadata.is_file()
        || path
            .extension()
            .is_none_or(|ext| !is_text_extension(ext.to_string_lossy().as_ref()))
    {
        return Ok(());
    }
    let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if size > 1_000_000 || !budget.visit_file(path, size) {
        return Ok(());
    }
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(());
    };
    let kind = if explicit { "context" } else { "source" };
    for (line_index, token) in term_tokens(&content) {
        let normalized = normalize(token);
        if normalized.len() < 3 {
            continue;
        }
        if !add_evidence(
            terms,
            budget,
            token,
            Evidence {
                path: path.display().to_string(),
                line: line_index,
                source_kind: kind,
                authoritative: true,
            },
        ) {
            break;
        }
    }
    Ok(())
}

fn add_evidence(
    terms: &mut HashMap<String, Term>,
    budget: &mut DiscoveryBudget,
    token: &str,
    evidence: Evidence,
) -> bool {
    let normalized = normalize(token);
    let is_new = !terms.contains_key(&normalized);
    if is_new && !budget.reserve_term() {
        return false;
    }
    if !budget.reserve_evidence() {
        return false;
    }
    terms
        .entry(normalized.clone())
        .or_insert_with(|| Term {
            display: token.to_owned(),
            normalized,
            evidence: Vec::new(),
        })
        .evidence
        .push(evidence);
    true
}

fn term_tokens(content: &str) -> impl Iterator<Item = (usize, &str)> {
    tokens(content).filter(|(_, token)| {
        token
            .chars()
            .any(|c| c == '.' || c == '_' || c.is_uppercase())
            && token.len() >= 3
    })
}

fn tokens(content: &str) -> impl Iterator<Item = (usize, &str)> {
    content.lines().enumerate().flat_map(|(line, text)| {
        text.split_whitespace().filter_map(move |token| {
            let token = token.trim_matches(|c: char| "\"'`()[]{}<>,;:".contains(c));
            (!token.is_empty()).then_some((line + 1, token))
        })
    })
}

fn phrase_context(
    transcript: &Transcript,
    index: usize,
    proposed: &str,
    observed: &str,
) -> (usize, String) {
    let Some(previous) = index.checked_sub(1).and_then(|i| transcript.words.get(i)) else {
        return (index, observed.to_owned());
    };
    let previous = previous
        .text
        .trim_matches(|c: char| c.is_ascii_punctuation());
    let first_component = proposed.split('.').next().unwrap_or_default();
    if !first_component.is_empty()
        && previous.eq_ignore_ascii_case(first_component)
        && proposed.contains('.')
        && observed.trim_start().starts_with('.')
    {
        (index - 1, format!("{} {}", previous, observed))
    } else {
        (index, observed.to_owned())
    }
}

fn collect_sibling_transcripts(
    root: &Path,
    current_output: &Path,
    terms: &mut HashMap<String, Term>,
    budget: &mut DiscoveryBudget,
) -> Result<()> {
    let cue_root = root.join(".cue");
    let Ok(entries) = fs::read_dir(cue_root) else {
        return Ok(());
    };
    for entry in entries {
        let workspace = entry
            .as_ref()
            .map_err(|e| {
                cue_core::CueError::general("could not inspect sibling workspace")
                    .because(e.to_string())
            })?
            .path();
        if workspace == current_output {
            continue;
        }
        let path = entry
            .map_err(|e| {
                cue_core::CueError::general("could not inspect sibling workspace")
                    .because(e.to_string())
            })?
            .path()
            .join("transcript.txt");
        if !path.is_file() {
            continue;
        }
        let size = fs::metadata(&path)
            .ok()
            .and_then(|metadata| usize::try_from(metadata.len()).ok())
            .unwrap_or(usize::MAX);
        if size > 1_000_000 || !budget.visit_file(&path, size) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        for (line_index, token) in term_tokens(&content) {
            if !add_evidence(
                terms,
                budget,
                token,
                Evidence {
                    path: path.display().to_string(),
                    line: line_index,
                    source_kind: "sibling-transcript",
                    authoritative: false,
                },
            ) {
                break;
            }
        }
        if budget.exhausted() {
            break;
        }
    }
    Ok(())
}

fn is_text_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "md" | "txt"
            | "rs"
            | "toml"
            | "json"
            | "yaml"
            | "yml"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "py"
            | "go"
            | "java"
            | "cs"
            | "rb"
            | "swift"
            | "kt"
            | "xml"
            | "ini"
            | "env"
    )
}
fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}
fn similarity(left: &str, right: &str) -> f32 {
    let left = normalize(left);
    let distance = levenshtein(&left, right);
    1.0 - (distance as f32 / left.len().max(right.len()) as f32)
}
fn levenshtein(left: &str, right: &str) -> usize {
    let mut row: Vec<usize> = (0..=right.len()).collect();
    for (i, a) in left.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, b) in right.chars().enumerate() {
            let current = row[j + 1];
            row[j + 1] = (row[j + 1] + 1)
                .min(row[j] + 1)
                .min(previous + usize::from(a != b));
            previous = current;
        }
    }
    row[right.len()]
}

fn source_for_workspace(workspace: &Path) -> Option<PathBuf> {
    let path = workspace.join("cue.workspace.json");
    let bytes = fs::read(&path).ok()?;
    #[derive(serde::Deserialize)]
    struct Descriptor {
        schema_version: u8,
        source: String,
    }
    let descriptor: Descriptor = serde_json::from_slice(&bytes).ok()?;
    if descriptor.schema_version != 1 {
        return None;
    }
    fs::canonicalize(workspace.join(descriptor.source)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_extension_typo_from_local_source() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let source = root.join("lesson.mp4");
        fs::write(&source, b"media").unwrap();
        let workspace = root.join(".cue/lesson");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(
            workspace.join("cue.workspace.json"),
            r#"{"schema_version":1,"source":"../../lesson.mp4"}"#,
        )
        .unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "name = \"demo\"\nmanifest = cargo.toml\n",
        )
        .unwrap();
        let transcript = Transcript {
            schema_version: 1,
            language: "en".into(),
            duration_ms: 100,
            words: vec![cue_core::Word {
                text: ".tomo,".into(),
                start_ms: 0,
                end_ms: 100,
                confidence: Some(0.7633),
                speaker: None,
            }],
            segments: vec![],
        };
        let candidates = find_candidates(&workspace, &transcript, 0.75, None).unwrap();
        assert_eq!(candidates[0].proposed, "cargo.toml");
    }

    #[test]
    fn discovery_budget_deduplicates_files_and_bounds_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("context.md");
        fs::write(&path, "Cargo.toml\n").unwrap();
        let size = fs::metadata(&path).unwrap().len() as usize;
        let mut budget = DiscoveryBudget::default();

        assert!(budget.visit_file(&path, size));
        assert!(!budget.visit_file(&path, size));
        assert_eq!(budget.visited_files.len(), 1);
        assert_eq!(budget.bytes, size);
    }
}
