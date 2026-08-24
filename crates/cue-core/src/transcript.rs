//! The canonical transcript: cue's source of truth.
//!
//! Raw timed words are kept verbatim from the transcriber. Cleaned text,
//! subtitles, and analysis all derive from this structure; nothing may
//! overwrite it.

use serde::{Deserialize, Serialize};

pub const TRANSCRIPT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// BCP-47-ish language tag as reported by the transcriber ("en", "de").
    pub language: String,
    pub duration_ms: u64,
    /// Every recognized word, in time order.
    pub words: Vec<Word>,
    /// Groupings of words into spoken segments; indices refer to `words`.
    pub segments: Vec<Segment>,
}

fn default_schema_version() -> u32 {
    TRANSCRIPT_SCHEMA_VERSION
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub confidence: Option<f32>,
    /// Reserved for future diarization; absent until then.
    pub speaker: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    /// Inclusive index of the first word of this segment.
    pub word_start: usize,
    /// Exclusive index one past the last word of this segment.
    pub word_end: usize,
}

impl Transcript {
    /// Render the plain-text form used for `transcript.txt`: one line per
    /// segment, preserving segment order.
    ///
    /// This is deliberately raw text (what was actually said); cleaned prose
    /// comes from normalization instead.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            out.push_str(segment.text.trim());
            out.push('\n');
        }
        // A trailing newline after every segment means empty transcripts get
        // exactly one stray newline; trim to keep output byte-stable.
        while out.ends_with("\n\n") {
            out.pop();
        }
        out
    }

    /// The full spoken text as one string, joining words with spaces where
    /// they do not carry their own punctuation.
    ///
    /// Subtitle segmentation uses this style of reconstruction when it needs
    /// sentence-level context beyond segment boundaries.
    pub fn full_text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start_ms: u64, end_ms: u64) -> Word {
        Word {
            text: text.into(),
            start_ms,
            end_ms,
            confidence: Some(0.95),
            speaker: None,
        }
    }

    fn sample() -> Transcript {
        Transcript {
            schema_version: TRANSCRIPT_SCHEMA_VERSION,
            language: "en".into(),
            duration_ms: 4_200,
            words: vec![
                word("hello", 0, 300),
                word("world,", 310, 600),
                word("this", 1_500, 1_700),
                word("is", 1_710, 1_850),
                word("cue.", 1_860, 2_100),
            ],
            segments: vec![
                Segment {
                    start_ms: 0,
                    end_ms: 600,
                    text: "hello world,".into(),
                    word_start: 0,
                    word_end: 2,
                },
                Segment {
                    start_ms: 1_500,
                    end_ms: 2_100,
                    text: "this is cue.".into(),
                    word_start: 2,
                    word_end: 5,
                },
            ],
        }
    }

    #[test]
    fn serializes_with_schema_version_and_defaults_on_load() {
        let json = serde_json::to_string(&sample()).unwrap();
        assert!(json.contains(r#""schema_version":1"#), "{json}");

        let stripped = json.replace(r#""schema_version":1,"#, "");
        let loaded: Transcript = serde_json::from_str(&stripped).unwrap();
        assert_eq!(loaded.schema_version, TRANSCRIPT_SCHEMA_VERSION);
    }

    #[test]
    fn plain_text_is_one_line_per_segment() {
        let text = sample().plain_text();
        assert_eq!(text, "hello world,\nthis is cue.\n");
    }

    #[test]
    fn plain_text_of_empty_transcript_is_empty() {
        let mut transcript = sample();
        transcript.segments.clear();
        assert_eq!(transcript.plain_text(), "");
    }

    #[test]
    fn full_text_joins_words_with_spaces() {
        assert_eq!(sample().full_text(), "hello world, this is cue.");
    }

    #[test]
    fn speaker_field_survives_round_trip() {
        let mut transcript = sample();
        transcript.words[0].speaker = Some("spk_0".into());
        let json = serde_json::to_string(&transcript).unwrap();
        let back: Transcript = serde_json::from_str(&json).unwrap();
        assert_eq!(back.words[0].speaker.as_deref(), Some("spk_0"));
    }

    #[test]
    fn deserializes_without_optional_fields() {
        let minimal = r#"{
            "language": "en",
            "duration_ms": 100,
            "words": [],
            "segments": []
        }"#;
        let transcript: Transcript = serde_json::from_str(minimal).unwrap();
        assert_eq!(transcript.schema_version, TRANSCRIPT_SCHEMA_VERSION);
        assert!(transcript.words.is_empty());
    }
}
