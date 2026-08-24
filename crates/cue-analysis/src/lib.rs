//! Structured analysis of transcripts through an LLM gateway.

pub mod analyzer;
pub mod json;
pub mod render;

pub use analyzer::{AnalysisInput, GatewayAnalyzer, Analyzer, PROMPT_VERSION};
pub use render::{render_description, render_summary};

#[cfg(test)]
mod tests {
    use super::render::*;
    use cue_core::{Analysis, Topic, ANALYSIS_SCHEMA_VERSION};

    fn sample() -> Analysis {
        Analysis {
            schema_version: ANALYSIS_SCHEMA_VERSION,
            language: "en".into(),
            title: "Building Cue".into(),
            summary: "A walkthrough of the pipeline.".into(),
            topics: vec![
                Topic {
                    start_ms: 0,
                    end_ms: 60_000,
                    title: "Introduction".into(),
                    summary: "Setting the stage.".into(),
                    key_points: vec!["Why local matters".into()],
                },
                Topic {
                    start_ms: 90_000,
                    end_ms: 150_000,
                    title: "Subtitles".into(),
                    summary: "Rendering SRT and VTT.".into(),
                    key_points: vec![],
                },
            ],
            key_points: vec![
                "Canonical transcript is the source of truth".into(),
                "Only cleaned text leaves the machine".into(),
            ],
            keywords: ["transcription", "local first"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    #[test]
    fn summary_has_title_body_and_points() {
        let md = render_summary(&sample());
        assert!(md.starts_with("# Building Cue\n"), "{md}");
        assert!(md.contains("walkthrough of the pipeline"), "{md}");
        assert!(md.contains("## Key points\n- Canonical transcript"), "{md}");
    }

    #[test]
    fn description_includes_chapter_timestamps_and_tags() {
        let md = render_description(&sample());
        assert!(md.contains("## Chapters"), "{md}");
        // 90_000 ms -> 01:30
        assert!(md.contains("- 01:30 Subtitles"), "{md}");
        assert!(md.contains("#transcription #localfirst"), "{md}");
        assert!(md.contains("- Only cleaned text leaves the machine"), "{md}");
    }

    #[test]
    fn empty_analysis_renders_without_sections() {
        let a = Analysis {
            schema_version: ANALYSIS_SCHEMA_VERSION,
            language: "en".into(),
            title: "T".into(),
            summary: "S".into(),
            ..Default::default()
        };
        let md = render_description(&a);
        assert!(!md.contains("## Chapters"));
        assert!(!md.contains("## Key points"));
    }
}
