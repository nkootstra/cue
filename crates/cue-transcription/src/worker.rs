//! The worker wire format and its mapping onto the canonical transcript.
//!
//! Keeping this pure (no I/O) makes the adapter contract testable without
//! spawning processes or models.

use cue_core::{Segment, Transcript, Word, TRANSCRIPT_SCHEMA_VERSION};
use serde::Deserialize;

/// Root object emitted by the faster-whisper worker on stdout.
#[derive(Debug, Deserialize)]
pub struct WorkerOutput {
    #[serde(default = "default_version")]
    pub version: u32,
    pub language: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub segments: Vec<WorkerSegment>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
pub struct WorkerSegment {
    #[serde(default)]
    pub id: u32,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub words: Option<Vec<WorkerWord>>,
}

#[derive(Debug, Deserialize)]
pub struct WorkerWord {
    pub word: String,
    pub start: Option<f64>,
    pub end: Option<f64>,
    #[serde(default)]
    pub probability: Option<f64>,
}

/// Seconds to milliseconds, rounded; negative or non-finite values clamp to
/// zero rather than poisoning the timeline.
pub fn seconds_to_ms(seconds: f64) -> u64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    (seconds * 1000.0).round() as u64
}

impl WorkerOutput {
    /// Map worker output onto the canonical transcript.
    ///
    /// Words missing timestamps fall back to their segment bounds so the
    /// word list stays time-ordered and total.
    pub fn into_transcript(self) -> Transcript {
        let language = self.language.unwrap_or_else(|| "unknown".to_string());
        let duration_ms = self.duration.map(seconds_to_ms).unwrap_or(0);

        let mut words: Vec<Word> = Vec::new();
        let mut segments: Vec<Segment> = Vec::new();

        for segment in self.segments {
            let seg_start = seconds_to_ms(segment.start);
            let seg_end = seconds_to_ms(segment.end);
            let seg_text = segment.text.clone().unwrap_or_default();
            let word_start = words.len();

            match segment.words {
                Some(worker_words) if !worker_words.is_empty() => {
                    for ww in worker_words {
                        words.push(Word {
                            text: ww.word.trim().to_string(),
                            start_ms: ww.start.map(seconds_to_ms).unwrap_or(seg_start),
                            end_ms: ww.end.map(seconds_to_ms).unwrap_or(seg_end),
                            confidence: ww.probability.map(|p| p as f32),
                            speaker: None,
                        });
                    }
                }
                _ => {
                    // Segment without word timings: synthesize a single word
                    // from the segment text so downstream consumers always
                    // see one word per segment at minimum.
                    for token in seg_text.split_whitespace() {
                        words.push(Word {
                            text: token.to_string(),
                            start_ms: seg_start,
                            end_ms: seg_end,
                            confidence: None,
                            speaker: None,
                        });
                    }
                }
            }

            let word_end = words.len();
            segments.push(Segment {
                start_ms: seg_start,
                end_ms: seg_end,
                text: seg_text,
                word_start,
                word_end,
            });
        }

        Transcript {
            schema_version: TRANSCRIPT_SCHEMA_VERSION,
            language,
            duration_ms,
            words,
            segments,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "version": 1,
        "language": "en",
        "duration": 4.2,
        "segments": [
            {
                "id": 0,
                "start": 0.0, "end": 0.6, "text": " hello world,",
                "words": [
                    {"word": " hello", "start": 0.0, "end": 0.3, "probability": 0.97},
                    {"word": " world,", "start": 0.31, "end": 0.6, "probability": 0.91}
                ]
            },
            {
                "id": 1,
                "start": 1.5, "end": 2.1, "text": " this is cue.",
                "words": [
                    {"word": " this", "start": 1.5, "end": 1.7, "probability": 0.88},
                    {"word": " is", "start": 1.71, "end": 1.85, "probability": 0.9},
                    {"word": " cue.", "start": 1.86, "end": 2.1, "probability": 0.85}
                ]
            }
        ]
    }"#;

    #[test]
    fn maps_fixture_onto_canonical_transcript() {
        let output: WorkerOutput = serde_json::from_str(FIXTURE).unwrap();
        let transcript = output.into_transcript();

        assert_eq!(transcript.schema_version, TRANSCRIPT_SCHEMA_VERSION);
        assert_eq!(transcript.language, "en");
        assert_eq!(transcript.duration_ms, 4_200);
        assert_eq!(transcript.words.len(), 5);
        assert_eq!(transcript.segments.len(), 2);

        // Word indices line up with segment boundaries.
        assert_eq!(transcript.segments[0].word_start, 0);
        assert_eq!(transcript.segments[0].word_end, 2);
        assert_eq!(transcript.segments[1].word_start, 2);
        assert_eq!(transcript.segments[1].word_end, 5);

        // Times are integer milliseconds; text is trimmed.
        assert_eq!(transcript.words[0], Word {
            text: "hello".into(),
            start_ms: 0,
            end_ms: 300,
            confidence: Some(0.97),
            speaker: None,
        });
        assert_eq!(transcript.words[1].start_ms, 310);
    }

    #[test]
    fn segments_without_words_synthesize_word_entries() {
        let json = r#"{
            "language": "en", "duration": 1.0,
            "segments": [{"id": 0, "start": 0.1, "end": 0.9, "text": " no word timings"}]
        }"#;
        let output: WorkerOutput = serde_json::from_str(json).unwrap();
        let transcript = output.into_transcript();

        assert_eq!(transcript.words.len(), 3);
        assert_eq!(transcript.segments[0].word_start, 0);
        assert_eq!(transcript.segments[0].word_end, 3);
        // Synthesized words inherit the segment bounds.
        assert_eq!(transcript.words[0].start_ms, 100);
        assert_eq!(transcript.words[2].end_ms, 900);
    }

    #[test]
    fn empty_output_maps_to_empty_transcript() {
        let output: WorkerOutput =
            serde_json::from_str(r#"{"language": null, "duration": 0}"#).unwrap();
        let transcript = output.into_transcript();
        assert_eq!(transcript.language, "unknown");
        assert!(transcript.words.is_empty());
        assert!(transcript.segments.is_empty());
    }

    #[test]
    fn seconds_convert_with_clamping() {
        assert_eq!(seconds_to_ms(0.3105), 311);
        assert_eq!(seconds_to_ms(-1.0), 0);
        assert_eq!(seconds_to_ms(f64::NAN), 0);
        assert_eq!(seconds_to_ms(61.5), 61_500);
    }

    #[test]
    fn unknown_fields_are_tolerated_for_forward_compatibility() {
        let json = r#"{"language": "en", "future_field": true, "segments": []}"#;
        let output: WorkerOutput = serde_json::from_str(json)
            .expect("worker v1 must tolerate unknown fields");
        assert_eq!(output.into_transcript().language, "en");
    }
}
