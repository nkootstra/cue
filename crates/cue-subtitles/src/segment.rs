//! Cue segmentation: grouping timed words into subtitle cues.
//!
//! The segmenter works on the canonical transcript only. Boundaries prefer,
//! in order: sentence punctuation, pauses between words, then the configured
//! character and duration budgets.

use cue_core::Transcript;

/// Policy knobs for segmentation and line breaking. Deliberately not
/// language-aware; callers derive it from configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct SubtitlePolicy {
    pub max_lines: usize,
    pub max_chars_per_line: usize,
    pub max_duration_ms: u64,
    /// Reading-speed ceiling in characters per second; `None` disables the
    /// check.
    pub max_chars_per_second: Option<f32>,
}

impl Default for SubtitlePolicy {
    fn default() -> Self {
        Self {
            max_lines: 2,
            max_chars_per_line: 42,
            max_duration_ms: 6_000,
            max_chars_per_second: None,
        }
    }
}

impl SubtitlePolicy {
    /// Total characters that fit on one cue's lines (newlines included).
    pub fn char_budget(&self) -> usize {
        self.max_lines * self.max_chars_per_line + self.max_lines.saturating_sub(1)
    }
}

/// One subtitle cue: a time range plus its text (not yet line-broken).
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
pub fn segment(transcript: &Transcript, policy: &SubtitlePolicy) -> Vec<Cue> {
    let mut cues = Vec::new();
    for spoken in &transcript.segments {
        let words = transcript.words.get(spoken.word_start..spoken.word_end);
        let Some(words) = words else { continue };
        segment_range(words, policy, &mut cues);
    }
    enforce_monotonic(cues)
}

fn segment_range(words: &[cue_core::Word], policy: &SubtitlePolicy, out: &mut Vec<Cue>) {
    const _: () = ();
    let budget = policy.char_budget();

    struct Pending {
        start_ms: u64,
        parts: Vec<String>,
        chars: usize,
    }

    let mut pending: Option<Pending> = None;
    let mut previous_end: Option<u64> = None;

    for word in words {
        let text = word.text.trim();
        if text.is_empty() {
            continue;
        }

        let pause_before = previous_end.map_or(0, |prev| word.start_ms.saturating_sub(prev));

        // Close the running cue when this word starts a new one anyway.
        if let Some(state) = &pending {
            let duration = word.start_ms.saturating_sub(state.start_ms);
            let over_budget = state.chars + 1 + text.len() > budget;
            let over_duration = duration > policy.max_duration_ms;
            let after_pause = pause_before >= PAUSE_BREAK_MS;

            if over_budget || over_duration || after_pause {
                let end = previous_end.unwrap_or(state.start_ms);
                out.push(Cue {
                    start_ms: state.start_ms,
                    end_ms: end.max(state.start_ms),
                    text: state.parts.join(" "),
                });
                pending = None;
            }
        }

        match &mut pending {
            None => {
                pending = Some(Pending {
                    start_ms: word.start_ms,
                    parts: vec![text.to_string()],
                    chars: text.chars().count(),
                });
            }
            Some(state) => {
                state.parts.push(text.to_string());
                state.chars += 1 + text.chars().count();
            }
        }
        previous_end = Some(word.end_ms);

        // Sentence punctuation closes immediately with this word's end.
        if ends_sentence(text)
            && let Some(state) = pending.take()
        {
            out.push(Cue {
                start_ms: state.start_ms,
                end_ms: word.end_ms.max(state.start_ms),
                text: state.parts.join(" "),
            });
        }
    }

    // Flush a trailing cue without sentence punctuation.
    if let Some(state) = pending.take() {
        out.push(Cue {
            start_ms: state.start_ms,
            end_ms: previous_end.unwrap_or(state.start_ms).max(state.start_ms),
            text: state.parts.join(" "),
        });
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
