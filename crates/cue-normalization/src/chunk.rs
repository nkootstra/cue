//! Chunking transcripts for normalization.
//!
//! S1-mini rewrites text; chunks bound how much it sees at once and keep
//! timestamp ranges attached so cleaned spans stay locatable in time.

use cue_core::Transcript;

/// A span of transcript text prepared for a normalizer, with the original
/// time range it covers.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptChunk {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// Split segment texts into chunks of at most `max_chars` characters.
///
/// Chunks break at segment boundaries; when one spoken segment alone
/// exceeds the budget it is split at word boundaries. Once a chunk reaches
/// half the budget, a sentence-final segment closes it — keeping prompts
/// aligned with natural sentence edges where possible.
pub fn chunk_transcript(transcript: &Transcript, max_chars: usize) -> Vec<TranscriptChunk> {
    let mut chunks = Vec::new();
    if max_chars == 0 {
        return chunks;
    }

    let mut acc = Accumulator::default();

    for spoken in &transcript.segments {
        let text = match spoken.text.trim() {
            "" => continue,
            t => t,
        };

        // Oversized single segment: split by words.
        if text.chars().count() > max_chars {
            acc.flush_into(&mut chunks);
            for piece in split_by_words(text, max_chars) {
                chunks.push(TranscriptChunk {
                    start_ms: spoken.start_ms,
                    end_ms: spoken.end_ms,
                    text: piece,
                });
            }
            continue;
        }

        let sentence_edge = ends_sentence(text);
        let would_overflow = acc.prospective_len(text) > max_chars;
        let over_half = acc.len() >= max_chars / 2;

        if !acc.is_empty() && (would_overflow || (sentence_edge && over_half)) {
            acc.flush_into(&mut chunks);
        }

        acc.push(spoken.start_ms, spoken.end_ms, text);

        if sentence_edge && acc.len() >= max_chars / 2 {
            acc.flush_into(&mut chunks);
        }
    }
    acc.flush_into(&mut chunks);

    chunks
}

struct PendingChunk {
    start_ms: u64,
    end_ms: u64,
    text: String,
}

#[derive(Default)]
struct Accumulator {
    pending: Option<PendingChunk>,
}

impl Accumulator {
    fn is_empty(&self) -> bool {
        self.pending.is_none()
    }

    fn len(&self) -> usize {
        self.pending
            .as_ref()
            .map_or(0, |pending| pending.text.chars().count())
    }

    /// Length if `next` were appended with a joining space.
    fn prospective_len(&self, next: &str) -> usize {
        let join = self.pending.is_some();
        self.len() + next.chars().count() + join as usize
    }

    fn push(&mut self, start_ms: u64, end_ms: u64, text: &str) {
        match &mut self.pending {
            Some(pending) => {
                pending.text.push(' ');
                pending.text.push_str(text);
                pending.end_ms = end_ms;
            }
            None => {
                self.pending = Some(PendingChunk {
                    start_ms,
                    end_ms,
                    text: text.to_string(),
                });
            }
        }
    }

    fn flush_into(&mut self, out: &mut Vec<TranscriptChunk>) {
        if let Some(pending) = self.pending.take() {
            out.push(TranscriptChunk {
                start_ms: pending.start_ms,
                end_ms: pending.end_ms,
                text: pending.text,
            });
        }
    }
}

fn ends_sentence(text: &str) -> bool {
    text.ends_with(['.', '!', '?'])
}

/// Word-boundary splitting for pathological segments.
fn split_by_words(text: &str, max_chars: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > max_chars {
            pieces.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    use cue_core::{Segment, TRANSCRIPT_SCHEMA_VERSION, Word};

    fn transcript(segments: &[(u64, u64, &str)]) -> Transcript {
        Transcript {
            schema_version: TRANSCRIPT_SCHEMA_VERSION,
            language: "en".into(),
            duration_ms: segments.last().map_or(0, |segment| segment.1),
            words: Vec::<Word>::new(),
            segments: segments
                .iter()
                .map(|(start_ms, end_ms, text)| Segment {
                    start_ms: *start_ms,
                    end_ms: *end_ms,
                    text: (*text).into(),
                    word_start: 0,
                    word_end: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn empty_transcript_and_zero_budget_produce_no_chunks() {
        assert!(chunk_transcript(&transcript(&[]), 100).is_empty());
        assert!(chunk_transcript(&transcript(&[(0, 10, "text")]), 0).is_empty());
    }

    #[test]
    fn oversized_segment_splits_without_losing_words() {
        let chunks = chunk_transcript(&transcript(&[(10, 90, "one two three four")]), 7);

        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect::<Vec<_>>(),
            ["one two", "three", "four"]
        );
        assert!(
            chunks
                .iter()
                .all(|chunk| (chunk.start_ms, chunk.end_ms) == (10, 90))
        );
    }

    #[test]
    fn trailing_pending_chunk_keeps_its_complete_time_range() {
        let chunks = chunk_transcript(&transcript(&[(100, 200, "one"), (250, 400, "two")]), 100);

        assert_eq!(
            chunks,
            [TranscriptChunk {
                start_ms: 100,
                end_ms: 400,
                text: "one two".into(),
            }]
        );
    }
}
