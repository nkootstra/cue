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

#[derive(Default)]
struct Accumulator {
    start_ms: Option<u64>,
    end_ms: u64,
    text: String,
}

impl Accumulator {
    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// Length if `next` were appended with a joining space.
    fn prospective_len(&self, next: &str) -> usize {
        let join = !self.text.is_empty();
        self.len() + next.chars().count() + join as usize
    }

    fn push(&mut self, start_ms: u64, end_ms: u64, text: &str) {
        if self.start_ms.is_none() {
            self.start_ms = Some(start_ms);
        }
        if !self.text.is_empty() {
            self.text.push(' ');
        }
        self.text.push_str(text);
        self.end_ms = end_ms;
    }

    fn flush_into(&mut self, out: &mut Vec<TranscriptChunk>) {
        if !self.text.is_empty() {
            out.push(TranscriptChunk {
                start_ms: self.start_ms.unwrap_or(0),
                end_ms: self.end_ms,
                text: std::mem::take(&mut self.text),
            });
        }
        self.start_ms = None;
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
        if !current.is_empty()
            && current.chars().count() + 1 + word.chars().count() > max_chars
        {
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
