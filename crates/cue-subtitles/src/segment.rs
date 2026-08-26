//! Cue segmentation: grouping timed words into subtitle cues.
//!
//! The segmenter works on the canonical transcript only. Boundaries prefer,
//! in order: sentence punctuation, pauses between words, then the configured
//! character and duration budgets.

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

/// A pause at least this long prefers a cue boundary before the next word.
const PAUSE_BREAK_MS: u64 = 700;

/// Sentence-final punctuation strongly prefers a boundary after the word.
fn ends_sentence(text: &str) -> bool {
    text.trim_end_matches(['"', '\'', '”', '’'])
        .ends_with(['.', '!', '?'])
}

/// Segment transcript words into cues under the policy.
pub fn segment(transcript: &Transcript, policy: &SubtitlePolicy) -> Result<Vec<Cue>> {
    transcript.validate()?;
    let mut cues = Vec::new();
    for spoken in &transcript.segments {
        let words = transcript.words_for_segment(spoken)?;
        segment_range(words, policy, &mut cues);
    }
    Ok(enforce_monotonic(cues))
}

fn segment_range(words: &[cue_core::Word], policy: &SubtitlePolicy, out: &mut Vec<Cue>) {
    struct PendingCue {
        start_ms: u64,
        end_ms: u64,
        layout: LineLayout,
    }

    impl PendingCue {
        fn new(word: &cue_core::Word, text: &str, policy: &SubtitlePolicy) -> Self {
            let mut layout = LineLayout::new(policy.max_lines, policy.max_chars_per_line);
            let appended = layout.try_append(text);
            debug_assert!(appended);
            Self {
                start_ms: word.start_ms,
                end_ms: word.end_ms.max(word.start_ms),
                layout,
            }
        }

        fn try_append(
            &mut self,
            word: &cue_core::Word,
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
            true
        }

        fn finish(self) -> Cue {
            Cue {
                start_ms: self.start_ms,
                end_ms: self.end_ms,
                text: self.layout.into_text(),
            }
        }
    }

    let mut pending: Option<PendingCue> = None;
    let mut previous_end: Option<u64> = None;

    for word in words {
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
            .is_some_and(|state| state.try_append(word, text, policy));
        if !appended {
            if let Some(state) = pending.take() {
                out.push(state.finish());
            }
            pending = Some(PendingCue::new(word, text, policy));
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
fn enforce_monotonic(mut cues: Vec<Cue>) -> Vec<Cue> {
    cues.sort_by_key(|c| c.start_ms);
    for i in 1..cues.len() {
        let prev_end = cues[i - 1].end_ms;
        if cues[i].start_ms < prev_end {
            cues[i - 1].end_ms = cues[i].start_ms.max(cues[i - 1].start_ms);
        }
    }
    cues.retain(|c| c.end_ms > c.start_ms);
    cues
}
