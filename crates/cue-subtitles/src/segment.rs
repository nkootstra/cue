//! Cue segmentation: grouping timed words into subtitle cues.
//!
//! The segmenter works on the canonical transcript only. Boundaries prefer,
//! in order: sentence punctuation, pauses between words, then the configured
//! character and duration budgets.

use std::collections::HashSet;

use cue_core::{Result, Transcript};

use crate::lines::LineLayout;

/// Policy knobs for segmentation and line breaking. Deliberately not
/// language-aware; callers derive it from configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct SubtitlePolicy {
    pub max_lines: usize,
    pub max_chars_per_line: usize,
    pub max_duration_ms: u64,
}

impl Default for SubtitlePolicy {
    fn default() -> Self {
        Self {
            max_lines: 2,
            max_chars_per_line: 42,
            max_duration_ms: 6_000,
        }
    }
}

/// One subtitle cue: a time range plus its final physical line layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// Half-open word range in the canonical transcript that produced a cue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CueSource {
    pub word_start: usize,
    pub word_end: usize,
}

/// A rendered cue paired with its canonical source address.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledCue {
    pub cue: Cue,
    pub source: CueSource,
}

/// The kind of timing intervention needed to keep generated cues valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingRepairKind {
    Shortened,
    Dropped,
}

/// A source-linked record of a timing intervention performed during compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingRepair {
    pub kind: TimingRepairKind,
    pub source: CueSource,
    pub start_ms: u64,
    pub original_end_ms: u64,
    pub repaired_end_ms: u64,
}

/// Generated cues plus any timing interventions applied to them.
#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleCompilation {
    pub cues: Vec<CompiledCue>,
    pub repairs: Vec<TimingRepair>,
}

/// A pause at least this long prefers a cue boundary before the next word.
const PAUSE_BREAK_MS: u64 = 700;

/// Sentence-final punctuation strongly prefers a boundary after the word.
fn ends_sentence(text: &str) -> bool {
    text.trim_end_matches(['"', '\'', '”', '’'])
        .ends_with(['.', '!', '?'])
}

/// Segment transcript words into cues under the policy.
pub fn segment(transcript: &Transcript, policy: &SubtitlePolicy) -> Result<Vec<Cue>> {
    Ok(compile(transcript, policy)?
        .cues
        .into_iter()
        .map(|compiled| compiled.cue)
        .collect())
}

/// Compile transcript words into source-addressed cues and timing evidence.
pub fn compile(transcript: &Transcript, policy: &SubtitlePolicy) -> Result<SubtitleCompilation> {
    let word_sources = (0..transcript.words.len())
        .map(|word_start| CueSource {
            word_start,
            word_end: word_start + 1,
        })
        .collect::<Vec<_>>();
    compile_with_sources(transcript, policy, &word_sources)
}

/// Compile using explicit canonical source ranges for each current word.
pub fn compile_with_sources(
    transcript: &Transcript,
    policy: &SubtitlePolicy,
    word_sources: &[CueSource],
) -> Result<SubtitleCompilation> {
    transcript.validate()?;
    if word_sources.len() != transcript.words.len()
        || word_sources.iter().any(|source| {
            source.word_start >= source.word_end || source.word_end > transcript.words.len()
        })
    {
        return Err(cue_core::CueError::general(
            "subtitle word source map does not match the transcript",
        ));
    }
    let mut cues = Vec::new();
    for spoken in &transcript.segments {
        let words = transcript.words_for_segment(spoken)?;
        let sources = &word_sources[spoken.word_start..spoken.word_end];
        segment_range(words, sources, policy, &mut cues);
    }
    Ok(enforce_monotonic(cues))
}

fn segment_range(
    words: &[cue_core::Word],
    word_sources: &[CueSource],
    policy: &SubtitlePolicy,
    out: &mut Vec<CompiledCue>,
) {
    struct PendingCue {
        start_ms: u64,
        end_ms: u64,
        source: CueSource,
        layout: LineLayout,
    }

    impl PendingCue {
        fn new(
            word: &cue_core::Word,
            source: CueSource,
            text: &str,
            policy: &SubtitlePolicy,
        ) -> Self {
            let mut layout = LineLayout::new(policy.max_lines, policy.max_chars_per_line);
            let appended = layout.try_append(text);
            debug_assert!(appended);
            Self {
                start_ms: word.start_ms,
                end_ms: word.end_ms.max(word.start_ms),
                source,
                layout,
            }
        }

        fn try_append(
            &mut self,
            word: &cue_core::Word,
            source: CueSource,
            text: &str,
            policy: &SubtitlePolicy,
        ) -> bool {
            let candidate_end = self.end_ms.max(word.end_ms).max(self.start_ms);
            if candidate_end.saturating_sub(self.start_ms) > policy.max_duration_ms {
                return false;
            }
            if !self.layout.try_append(text) {
                return false;
            }
            self.end_ms = candidate_end;
            self.source.word_start = self.source.word_start.min(source.word_start);
            self.source.word_end = self.source.word_end.max(source.word_end);
            true
        }

        fn finish(self) -> CompiledCue {
            CompiledCue {
                cue: Cue {
                    start_ms: self.start_ms,
                    end_ms: self.end_ms,
                    text: self.layout.into_text(),
                },
                source: self.source,
            }
        }
    }

    let mut pending: Option<PendingCue> = None;
    let mut previous_end: Option<u64> = None;

    for (word, source) in words.iter().zip(word_sources.iter().copied()) {
        let text = word.text.trim();
        if text.is_empty() {
            continue;
        }

        let pause_before = previous_end.map_or(0, |prev| word.start_ms.saturating_sub(prev));

        if pause_before >= PAUSE_BREAK_MS
            && let Some(state) = pending.take()
        {
            out.push(state.finish());
        }

        let appended = pending
            .as_mut()
            .is_some_and(|state| state.try_append(word, source, text, policy));
        if !appended {
            if let Some(state) = pending.take() {
                out.push(state.finish());
            }
            pending = Some(PendingCue::new(word, source, text, policy));
        }
        previous_end = Some(word.end_ms);

        // Sentence punctuation closes immediately with this word's end.
        if ends_sentence(text)
            && let Some(state) = pending.take()
        {
            out.push(state.finish());
        }
    }

    // Flush a trailing cue without sentence punctuation.
    if let Some(state) = pending.take() {
        out.push(state.finish());
    }
}

/// Clamp overlaps so consecutive cues never cross, keeping output valid.
fn enforce_monotonic(mut cues: Vec<CompiledCue>) -> SubtitleCompilation {
    cues.sort_by_key(|compiled| compiled.cue.start_ms);
    let mut repairs = Vec::new();
    for i in 1..cues.len() {
        let next_start = cues[i].cue.start_ms;
        let previous = &mut cues[i - 1];
        if next_start < previous.cue.end_ms {
            let original_start_ms = previous.cue.start_ms;
            let original_end_ms = previous.cue.end_ms;
            let repaired_end_ms = next_start.max(original_start_ms);
            previous.cue.end_ms = repaired_end_ms;
            repairs.push(TimingRepair {
                kind: if repaired_end_ms == original_start_ms {
                    TimingRepairKind::Dropped
                } else {
                    TimingRepairKind::Shortened
                },
                source: previous.source,
                start_ms: original_start_ms,
                original_end_ms,
                repaired_end_ms,
            });
        }
    }
    let overlap_drops = repairs
        .iter()
        .filter(|repair| repair.kind == TimingRepairKind::Dropped)
        .map(|repair| repair.source)
        .collect::<HashSet<_>>();
    cues.retain(|compiled| {
        if compiled.cue.end_ms > compiled.cue.start_ms {
            return true;
        }
        if !overlap_drops.contains(&compiled.source) {
            repairs.push(TimingRepair {
                kind: TimingRepairKind::Dropped,
                source: compiled.source,
                start_ms: compiled.cue.start_ms,
                original_end_ms: compiled.cue.end_ms,
                repaired_end_ms: compiled.cue.end_ms,
            });
        }
        false
    });
    SubtitleCompilation { cues, repairs }
}
