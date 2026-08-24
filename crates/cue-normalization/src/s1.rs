//! The S1-mini normalizer running through Ollama.
//!
//! S1 is a single-purpose text normalizer, not a chat model. Prompts follow
//! its documented control-line format; decoding is greedy (temperature 0)
//! and enforced by the Modelfile the model is created from.

use async_trait::async_trait;
use cue_core::{NormalizedChunk, NormalizedTranscript, Transcript};
use tracing::instrument;

use crate::TranscriptNormalizer;
use crate::chunk::{TranscriptChunk, chunk_transcript};

/// Styling knobs from S1's control line.
#[derive(Debug, Clone, PartialEq)]
pub struct S1Settings {
    pub styling: String,
    pub structure: String,
    pub context: String,
}

impl Default for S1Settings {
    fn default() -> Self {
        Self {
            styling: "semi-formal".into(),
            structure: "prose".into(),
            context: "general".into(),
        }
    }
}

impl S1Settings {
    fn control_line(&self) -> String {
        format!(
            "[Styling: {}] [Structure: {}] [Context: {}]",
            self.styling, self.structure, self.context
        )
    }
}

/// Normalizes via an OpenAI-compatible chat endpoint backed by Ollama.
pub struct S1Normalizer {
    client: cue_llm::ChatClient,
    model: String,
    settings: S1Settings,
    max_chunk_chars: usize,
}

impl S1Normalizer {
    /// Point at Ollama's OpenAI-compatible endpoint (`<ollama_url>/v1`).
    pub fn new(ollama_url: &str) -> Self {
        Self {
            client: cue_llm::ChatClient::new(
                format!("{}/v1", ollama_url.trim_end_matches('/')),
                None,
            ),
            model: crate::S1_MODEL_NAME.to_string(),
            settings: S1Settings::default(),
            max_chunk_chars: 2_000,
        }
    }

    pub fn with_settings(mut self, settings: S1Settings) -> Self {
        self.settings = settings;
        self
    }

    pub fn with_chunk_limit(mut self, max_chunk_chars: usize) -> Self {
        self.max_chunk_chars = max_chunk_chars;
        self
    }

    /// Prompt for one chunk: control line plus raw text.
    fn prompt_for(&self, chunk: &TranscriptChunk) -> String {
        format!("{}\n{}", self.settings.control_line(), chunk.text)
    }
}

#[async_trait]
impl TranscriptNormalizer for S1Normalizer {
    fn name(&self) -> &str {
        "s1"
    }

    #[instrument(skip(self, transcript))]
    async fn normalize(&self, transcript: &Transcript) -> cue_core::Result<NormalizedTranscript> {
        let chunks = chunk_transcript(transcript, self.max_chunk_chars);
        let mut normalized = Vec::with_capacity(chunks.len());

        for chunk in chunks {
            let response = self
                .client
                .chat(
                    &self.model,
                    &[cue_llm::ChatMessage::user(self.prompt_for(&chunk))],
                    Some(0.0), // greedy decode; normalization is deterministic
                )
                .await?;

            normalized.push(NormalizedChunk {
                start_ms: chunk.start_ms,
                end_ms: chunk.end_ms,
                text: response.content.trim().to_string(),
            });
        }

        Ok(NormalizedTranscript {
            schema_version: cue_core::NORMALIZED_SCHEMA_VERSION,
            chunks: normalized,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cue_core::{Segment, Word};
    use wiremock::matchers::{body_partial_json, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn transcript_with_segments(texts: &[&str]) -> Transcript {
        let mut words = Vec::new();
        let mut segments = Vec::new();
        let mut ms = 0u64;
        for (i, text) in texts.iter().enumerate() {
            let word_count = text.split_whitespace().count();
            for _ in 0..word_count {
                words.push(Word {
                    text: "w".into(),
                    start_ms: ms,
                    end_ms: ms + 100,
                    confidence: None,
                    speaker: None,
                });
                ms += 100;
            }
            segments.push(Segment {
                start_ms: ms - (word_count as u64 * 100),
                end_ms: ms,
                text: (*text).into(),
                word_start: 0,
                word_end: 0,
            });
            let _ = i;
        }
        Transcript {
            schema_version: cue_core::TRANSCRIPT_SCHEMA_VERSION,
            language: "en".into(),
            duration_ms: ms,
            words,
            segments,
        }
    }

    #[tokio::test]
    async fn normalizes_each_chunk_and_preserves_time_ranges() {
        let server = MockServer::start().await;

        // Respond with a recognizable transformation.
        Mock::given(method("POST"))
            .and(body_partial_json(serde_json::json!({
                "model": "cue-s1-mini",
                "messages": [{"role": "user",
                              "content": "[Styling: semi-formal] [Structure: prose] [Context: general]\nfirst bit."}]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"choices":[{"message":{"role":"assistant","content":"First bit."}}]}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"choices":[{"message":{"role":"assistant","content":"Second bit."}}]}"#,
            ))
            .mount(&server)
            .await;

        // Two sentences well past half the budget close into two chunks.
        let t = transcript_with_segments(&["first bit.", "second bit."]);
        let normalizer = S1Normalizer::new(&server.uri()).with_chunk_limit(20);

        let result = normalizer.normalize(&t).await.unwrap();

        assert_eq!(result.chunks.len(), 2);
        assert_eq!(result.chunks[0].text, "First bit.");
        assert!(result.chunks[0].end_ms <= result.chunks[1].start_ms);
    }

    #[tokio::test]
    async fn empty_transcript_short_circuits_without_network() {
        // Port 9 discards traffic; success proves no request was made.
        let normalizer = S1Normalizer::new("http://127.0.0.1:9");
        let t = transcript_with_segments(&[]);
        let result = normalizer.normalize(&t).await.unwrap();
        assert!(result.chunks.is_empty());
    }

    #[test]
    fn control_line_follows_s1_format() {
        assert_eq!(
            S1Settings::default().control_line(),
            "[Styling: semi-formal] [Structure: prose] [Context: general]"
        );
    }
}
