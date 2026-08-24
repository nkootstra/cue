//! The structured analysis of a video: understanding, before presentation.
//!
//! All human-readable outputs (summary.md, description.md, chapters)
//! derive from this representation; nothing renders without it.

use serde::{Deserialize, Serialize};

pub const ANALYSIS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub language: String,
    pub title: String,
    pub summary: String,
    /// Grounded chapter-like spans covering the content.
    #[serde(default)]
    pub topics: Vec<Topic>,
    #[serde(default)]
    pub key_points: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

fn default_schema_version() -> u32 {
    ANALYSIS_SCHEMA_VERSION
}

impl Default for Analysis {
    fn default() -> Self {
        Self {
            schema_version: ANALYSIS_SCHEMA_VERSION,
            language: String::new(),
            title: String::new(),
            summary: String::new(),
            topics: Vec::new(),
            key_points: Vec::new(),
            keywords: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Topic {
    pub start_ms: u64,
    pub end_ms: u64,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub key_points: Vec<String>,
}

impl Analysis {
    /// Chapters usable by players: topics sorted and clamped to valid ranges.
    pub fn chapters(&self) -> Vec<(&str, u64)> {
        let mut points: Vec<(&str, u64)> = self
            .topics
            .iter()
            .filter(|t| t.title.trim().len() > 0 && t.start_ms < t.end_ms)
            .map(|t| (t.title.as_str(), t.start_ms))
            .collect();
        points.sort_by_key(|(_, ms)| *ms);
        points.dedup_by_key(|(_, ms)| *ms);
        points
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Analysis {
        Analysis {
            schema_version: ANALYSIS_SCHEMA_VERSION,
            language: "en".into(),
            title: "Building Cue".into(),
            summary: "A walkthrough of the cue pipeline.".into(),
            topics: vec![
                Topic {
                    start_ms: 120_000,
                    end_ms: 240_000,
                    title: "Subtitles".into(),
                    summary: String::new(),
                    key_points: vec![],
                },
                Topic {
                    start_ms: 0,
                    end_ms: 119_000,
                    title: "Introduction".into(),
                    summary: String::new(),
                    key_points: vec![],
                },
                Topic {
                    start_ms: 500,
                    end_ms: 500,
                    title: "Empty span".into(), // invalid: filtered out
                    summary: String::new(),
                    key_points: vec![],
                },
            ],
            key_points: vec!["Local-first processing".into()],
            keywords: vec!["transcription".into(), "subtitles".into()],
        }
    }

    #[test]
    fn serializes_with_schema_version() {
        let json = serde_json::to_string(&sample()).unwrap();
        assert!(json.contains(r#""schema_version":1"#), "{json}");
        assert!(json.contains(r#""keywords""#), "{json}");
    }

    #[test]
    fn loads_without_optional_fields() {
        let json = r#"{
            "language": "en",
            "title": "T",
            "summary": "S"
        }"#;
        let a: Analysis = serde_json::from_str(json).unwrap();
        assert_eq!(a.schema_version, ANALYSIS_SCHEMA_VERSION);
        assert!(a.topics.is_empty());
    }

    #[test]
    fn chapters_are_sorted_and_skip_invalid_spans() {
        let chapters = sample().chapters();
        assert_eq!(chapters.len(), 2, "empty-span topic must be dropped");
        assert_eq!(chapters[0].0, "Introduction");
        assert_eq!(chapters[1].0, "Subtitles");
    }
}
