//! Normalized transcripts produced by S1 or similar cleanup models.
//!
//! Normalization rewrites text but must never be confused with the
//! canonical transcript: word-level timing is lost by design, so chunks
//! carry coarse timestamp ranges instead.

use serde::{Deserialize, Serialize};

pub const NORMALIZED_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedTranscript {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Cleaned text spans, time-ordered.
    pub chunks: Vec<NormalizedChunk>,
}

fn default_schema_version() -> u32 {
    NORMALIZED_SCHEMA_VERSION
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedChunk {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

impl Default for NormalizedTranscript {
    fn default() -> Self {
        Self {
            schema_version: NORMALIZED_SCHEMA_VERSION,
            chunks: Vec::new(),
        }
    }
}

impl NormalizedTranscript {
    /// Render `transcript.clean.txt`: one paragraph per non-empty chunk.
    pub fn plain_text(&self) -> String {
        self.chunks
            .iter()
            .map(|c| c.text.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
            + if self.chunks.is_empty() { "" } else { "\n" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_joins_chunks_as_paragraphs() {
        let t = NormalizedTranscript {
            schema_version: NORMALIZED_SCHEMA_VERSION,
            chunks: vec![
                NormalizedChunk {
                    start_ms: 0,
                    end_ms: 100,
                    text: "First thought.".into(),
                },
                NormalizedChunk {
                    start_ms: 200,
                    end_ms: 300,
                    text: "Second thought.".into(),
                },
            ],
        };
        assert_eq!(t.plain_text(), "First thought.\n\nSecond thought.\n");
    }

    #[test]
    fn empty_chunks_render_empty_and_load_without_version() {
        assert_eq!(NormalizedTranscript::default().plain_text(), "");

        let t: NormalizedTranscript =
            serde_json::from_str(r#"{"chunks":[]}"#).unwrap();
        assert_eq!(t.schema_version, NORMALIZED_SCHEMA_VERSION);
    }
}
