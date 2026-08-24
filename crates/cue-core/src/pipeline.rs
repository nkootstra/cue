use serde::Serialize;

/// The ordered stages of the cue processing pipeline.
///
/// Each stage is independently cacheable and restartable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PipelineStage {
    Inspect,
    Extract,
    Transcribe,
    Caption,
    Normalize,
    Analyze,
    Render,
}

impl PipelineStage {
    pub const ALL: [PipelineStage; 7] = [
        PipelineStage::Inspect,
        PipelineStage::Extract,
        PipelineStage::Transcribe,
        PipelineStage::Caption,
        PipelineStage::Normalize,
        PipelineStage::Analyze,
        PipelineStage::Render,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PipelineStage::Inspect => "inspect",
            PipelineStage::Extract => "extract",
            PipelineStage::Transcribe => "transcribe",
            PipelineStage::Caption => "caption",
            PipelineStage::Normalize => "normalize",
            PipelineStage::Analyze => "analyze",
            PipelineStage::Render => "render",
        }
    }
}

impl std::fmt::Display for PipelineStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Events emitted while the pipeline runs.
///
/// The core pipeline emits these; only the CLI renders them (e.g. via
/// indicatif). The pipeline itself never touches the terminal.
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineEvent {
    Started(PipelineStage),
    Progress {
        stage: PipelineStage,
        current: u64,
        total: Option<u64>,
    },
    /// A stage was satisfied from cache without doing work.
    Cached(PipelineStage),
    Completed(PipelineStage),
    Failed {
        stage: PipelineStage,
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_are_ordered_like_the_pipeline() {
        let names: Vec<_> = PipelineStage::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            [
                "inspect",
                "extract",
                "transcribe",
                "caption",
                "normalize",
                "analyze",
                "render"
            ]
        );
    }

    #[test]
    fn stage_serializes_lowercase() {
        let json = serde_json::to_string(&PipelineStage::Normalize).unwrap();
        assert_eq!(json, r#""normalize""#);
    }

    #[test]
    fn event_display_shapes_are_distinct() {
        let started = PipelineEvent::Started(PipelineStage::Extract);
        let cached = PipelineEvent::Cached(PipelineStage::Extract);
        assert_ne!(started, cached);

        let progress = PipelineEvent::Progress {
            stage: PipelineStage::Transcribe,
            current: 5,
            total: Some(10),
        };
        assert!(matches!(
            progress,
            PipelineEvent::Progress { current: 5, .. }
        ));

        let failed = PipelineEvent::Failed {
            stage: PipelineStage::Analyze,
            error: "gateway unreachable".into(),
        };
        assert!(format!("{failed:?}").contains("gateway unreachable"));
    }
}
