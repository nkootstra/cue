//! Focused correction review for canonical transcripts.

use std::path::Path;

use cue_core::{CueError, Result};

use crate::cli::ReviewArgs;
use crate::corrections::{CorrectionPlan, manifest_reference};
use crate::render::println_line;

#[derive(serde::Serialize)]
struct ReviewReport {
    schema_version: u8,
    output: String,
    confidence_below: f32,
    diagnostics: Vec<ReviewDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accepted: Option<AcceptedCandidate>,
}

#[derive(serde::Serialize)]
struct AcceptedCandidate {
    id: String,
    find: String,
    replace: String,
    manifest: String,
}

#[derive(serde::Serialize)]
#[serde(tag = "id")]
enum ReviewDiagnostic {
    #[serde(rename = "CUE-REVIEW-LOW-CONFIDENCE")]
    LowConfidence {
        word: String,
        word_index: usize,
        confidence: f32,
        start_ms: u64,
        end_ms: u64,
    },
    #[serde(rename = "CUE-REVIEW-POSSIBLE-FALLBACK-TIMING")]
    PossibleFallbackTiming {
        word: String,
        word_index: usize,
        start_ms: u64,
        end_ms: u64,
    },
    #[serde(rename = "CUE-REVIEW-UNMATCHED-RULE")]
    UnmatchedRule {
        find: String,
        replace: String,
        manifest: String,
    },
    #[serde(rename = "CUE-REVIEW-SCOPE-CONFLICT")]
    ScopeConflict {
        find: String,
        winner: String,
        shadowed: String,
        winner_manifest: String,
        shadowed_manifest: String,
    },
    #[serde(rename = "CUE-REVIEW-AMBIGUOUS-SPEAKER-TURN")]
    AmbiguousSpeakerTurn {
        segment_index: usize,
        speakers: Vec<String>,
        has_unassigned_words: bool,
    },
    #[serde(rename = "CUE-REVIEW-TERM-MISMATCH")]
    TermMismatch {
        candidate_id: String,
        observed: String,
        proposed: String,
        word_index: usize,
        confidence: Option<f32>,
        score: f32,
        evidence: Vec<crate::terminology::Evidence>,
    },
}

impl ReviewDiagnostic {
    fn id(&self) -> &'static str {
        match self {
            Self::LowConfidence { .. } => "CUE-REVIEW-LOW-CONFIDENCE",
            Self::PossibleFallbackTiming { .. } => "CUE-REVIEW-POSSIBLE-FALLBACK-TIMING",
            Self::UnmatchedRule { .. } => "CUE-REVIEW-UNMATCHED-RULE",
            Self::ScopeConflict { .. } => "CUE-REVIEW-SCOPE-CONFLICT",
            Self::AmbiguousSpeakerTurn { .. } => "CUE-REVIEW-AMBIGUOUS-SPEAKER-TURN",
            Self::TermMismatch { .. } => "CUE-REVIEW-TERM-MISMATCH",
        }
    }

    fn message(&self) -> String {
        match self {
            Self::LowConfidence {
                word, confidence, ..
            } => format!("{word:?} has confidence {confidence:.3}"),
            Self::PossibleFallbackTiming { word, .. } => {
                format!("{word:?} may be using segment-level fallback timing")
            }
            Self::UnmatchedRule { find, manifest, .. } => {
                format!("rule {find:?} from {manifest} did not match the canonical transcript")
            }
            Self::ScopeConflict {
                find,
                winner,
                shadowed,
                ..
            } => format!("rule {find:?} resolves to {winner:?}, shadowing {shadowed:?}"),
            Self::AmbiguousSpeakerTurn { segment_index, .. } => {
                format!("segment {segment_index} has ambiguous speaker assignments")
            }
            Self::TermMismatch {
                candidate_id,
                observed,
                proposed,
                ..
            } => {
                format!(
                    "possible terminology mismatch [{candidate_id}]: {observed:?} -> {proposed:?}"
                )
            }
        }
    }
}

pub fn run(args: ReviewArgs, corrections: Option<&Path>, output_root: Option<&Path>) -> i32 {
    let result = crate::commands::correct::resolve_output_dir_at(&args.output, output_root)
        .and_then(|output_dir| {
            let mut report = review_output(
                &output_dir,
                corrections,
                args.confidence_below,
                !args.no_terms,
                args.context_root.as_deref(),
            )?;
            if let Some(id) = args.accept.as_deref() {
                let _output_lock = crate::run_contract::OutputLock::acquire(&output_dir)?;
                report.accepted = Some(accept_candidate(&output_dir, &report, id)?);
            }
            Ok(report)
        });

    match result {
        Ok(report) if args.json => match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                println_line(&json);
                0
            }
            Err(error) => {
                eprintln!("could not serialize review report: {error}");
                1
            }
        },
        Ok(report) => {
            if report.diagnostics.is_empty() {
                println_line("No correction candidates found.");
            } else {
                for diagnostic in &report.diagnostics {
                    println_line(&format!("{}: {}", diagnostic.id(), diagnostic.message()));
                }
                println_line(&format!(
                    "\n{} correction candidate(s) found.",
                    report.diagnostics.len()
                ));
            }
            if let Some(accepted) = &report.accepted {
                println_line(&format!(
                    "Accepted {}: {} -> {}",
                    accepted.id, accepted.find, accepted.replace
                ));
                println_line("Run `cue correct` for this output to rebuild derived artifacts.");
            }
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn review_output(
    output_dir: &Path,
    explicit: Option<&Path>,
    confidence_below: f32,
    terms_enabled: bool,
    context_root: Option<&Path>,
) -> Result<ReviewReport> {
    if !(0.0..=1.0).contains(&confidence_below) {
        return Err(CueError::general(
            "--confidence-below must be between 0.0 and 1.0",
        ));
    }
    let transcript_path = output_dir.join("transcript.json");
    let transcript_bytes = std::fs::read(&transcript_path).map_err(|error| {
        CueError::general(format!("could not read {}", transcript_path.display()))
            .because(error.to_string())
    })?;
    let transcript: cue_core::Transcript =
        serde_json::from_slice(&transcript_bytes).map_err(|error| {
            CueError::general(format!("could not parse {}", transcript_path.display()))
                .because(error.to_string())
        })?;
    transcript.validate()?;
    let plan = CorrectionPlan::prepare(output_dir, explicit)?;
    let mut diagnostics = Vec::new();

    for (word_index, word) in transcript.words.iter().enumerate() {
        match word.confidence {
            Some(confidence) if confidence < confidence_below => {
                diagnostics.push(ReviewDiagnostic::LowConfidence {
                    word: word.text.clone(),
                    word_index,
                    confidence,
                    start_ms: word.start_ms,
                    end_ms: word.end_ms,
                });
            }
            _ => {}
        }
    }

    if terms_enabled {
        for candidate in crate::terminology::find_candidates(
            output_dir,
            &transcript,
            confidence_below,
            context_root,
        )? {
            diagnostics.push(ReviewDiagnostic::TermMismatch {
                candidate_id: candidate.id,
                observed: candidate.observed,
                proposed: candidate.proposed,
                word_index: candidate.word_index,
                confidence: candidate.confidence,
                score: candidate.score,
                evidence: candidate.evidence,
            });
        }
    }

    for (segment_index, segment) in transcript.segments.iter().enumerate() {
        let words = transcript.words_for_segment(segment)?;
        if words.len() > 1
            && words
                .iter()
                .all(|word| word.start_ms == segment.start_ms && word.end_ms == segment.end_ms)
        {
            for (offset, word) in words.iter().enumerate() {
                diagnostics.push(ReviewDiagnostic::PossibleFallbackTiming {
                    word: word.text.clone(),
                    word_index: segment.word_start + offset,
                    start_ms: word.start_ms,
                    end_ms: word.end_ms,
                });
            }
        }
        let mut speakers = words
            .iter()
            .filter_map(|word| word.speaker.as_deref())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        speakers.sort();
        speakers.dedup();
        let has_unassigned_words = words.iter().any(|word| word.speaker.is_none());
        if speakers.len() > 1 || (!speakers.is_empty() && has_unassigned_words) {
            diagnostics.push(ReviewDiagnostic::AmbiguousSpeakerTurn {
                segment_index,
                speakers,
                has_unassigned_words,
            });
        }
    }

    if let CorrectionPlan::Apply(prepared) = plan {
        let text = transcript.plain_text();
        let rules = prepared
            .rules
            .iter()
            .map(|rule| rule.correction.clone())
            .collect::<Vec<_>>();
        let matched = cue_core::correct::matched_rules(&text, &rules)
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let references = prepared
            .manifests
            .iter()
            .map(|manifest| manifest_reference(output_dir, &manifest.path))
            .collect::<Vec<_>>();
        for (index, rule) in prepared.rules.iter().enumerate() {
            if !matched.contains(&index) {
                diagnostics.push(ReviewDiagnostic::UnmatchedRule {
                    find: rule.correction.old.clone(),
                    replace: rule.correction.new.clone(),
                    manifest: references[rule.source_manifest].clone(),
                });
            }
        }
        for conflict in &prepared.conflicts {
            diagnostics.push(ReviewDiagnostic::ScopeConflict {
                find: conflict.find.clone(),
                winner: conflict.winner.clone(),
                shadowed: conflict.shadowed.clone(),
                winner_manifest: references[conflict.winner_manifest].clone(),
                shadowed_manifest: references[conflict.shadowed_manifest].clone(),
            });
        }
    }

    Ok(ReviewReport {
        schema_version: 2,
        output: output_dir.to_string_lossy().into_owned(),
        confidence_below,
        diagnostics,
        accepted: None,
    })
}

fn accept_candidate(
    output_dir: &Path,
    report: &ReviewReport,
    id: &str,
) -> Result<AcceptedCandidate> {
    let ReviewDiagnostic::TermMismatch { observed, proposed, candidate_id, .. } = report
        .diagnostics
        .iter()
        .find(|diagnostic| matches!(diagnostic, ReviewDiagnostic::TermMismatch { candidate_id, .. } if candidate_id == id))
        .ok_or_else(|| CueError::general(format!("unknown or stale terminology candidate {id:?}")))? else {
        unreachable!()
    };
    let manifest = output_dir.join("corrections.md");
    let mut content = match std::fs::read_to_string(&manifest) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(
                CueError::general(format!("could not read {}", manifest.display()))
                    .because(error.to_string()),
            );
        }
    };
    let find = observed.trim_end_matches(|c: char| c.is_ascii_punctuation());
    let rule = format!("{} -> {}", find, proposed);
    let existing = cue_core::correct::parse_manifest(&content)?;
    if let Some(current) = existing
        .iter()
        .find(|current| current.old.eq_ignore_ascii_case(find))
    {
        if !current.new.eq_ignore_ascii_case(proposed) {
            return Err(CueError::general(format!(
                "corrections.md already maps {find:?} to {:?}",
                current.new
            )));
        }
    } else {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&rule);
        content.push('\n');
        crate::run_contract::write_atomic(&manifest, content.as_bytes())?;
    }
    Ok(AcceptedCandidate {
        id: candidate_id.clone(),
        find: find.to_owned(),
        replace: proposed.clone(),
        manifest: manifest.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::ReviewDiagnostic;

    #[test]
    fn serialized_diagnostic_ids_match_display_ids() {
        let diagnostics = [
            ReviewDiagnostic::LowConfidence {
                word: "word".to_owned(),
                word_index: 0,
                confidence: 0.5,
                start_ms: 0,
                end_ms: 100,
            },
            ReviewDiagnostic::PossibleFallbackTiming {
                word: "word".to_owned(),
                word_index: 0,
                start_ms: 0,
                end_ms: 100,
            },
            ReviewDiagnostic::UnmatchedRule {
                find: "old".to_owned(),
                replace: "new".to_owned(),
                manifest: "corrections.md".to_owned(),
            },
            ReviewDiagnostic::ScopeConflict {
                find: "old".to_owned(),
                winner: "near".to_owned(),
                shadowed: "broad".to_owned(),
                winner_manifest: "course/corrections.md".to_owned(),
                shadowed_manifest: "corrections.md".to_owned(),
            },
            ReviewDiagnostic::AmbiguousSpeakerTurn {
                segment_index: 0,
                speakers: vec!["speaker-1".to_owned(), "speaker-2".to_owned()],
                has_unassigned_words: false,
            },
            ReviewDiagnostic::TermMismatch {
                candidate_id: "term-0".to_owned(),
                observed: ".tomo".to_owned(),
                proposed: "cargo.toml".to_owned(),
                word_index: 0,
                confidence: Some(0.7),
                score: 0.75,
                evidence: Vec::new(),
            },
        ];

        for diagnostic in diagnostics {
            let serialized = serde_json::to_value(&diagnostic).unwrap();
            assert_eq!(serialized["id"], diagnostic.id());
        }
    }

    #[test]
    fn terminology_message_includes_acceptance_id() {
        let diagnostic = ReviewDiagnostic::TermMismatch {
            candidate_id: "term-7".to_owned(),
            observed: ".tomo".to_owned(),
            proposed: "cargo.toml".to_owned(),
            word_index: 7,
            confidence: Some(0.7),
            score: 0.75,
            evidence: Vec::new(),
        };

        assert_eq!(
            diagnostic.message(),
            "possible terminology mismatch [term-7]: \".tomo\" -> \"cargo.toml\""
        );
    }
}
