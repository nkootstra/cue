use crate::pipeline::PipelineStage;

/// The allowlisted part of an error that is safe to store in recovery state.
///
/// Causes are deliberately excluded because provider output can contain
/// credentials or media-derived text.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistentFailure {
    pub stage: Option<PipelineStage>,
    pub summary: String,
    pub remedy: Option<String>,
}

/// The single error type for cue.
///
/// Every error explains four things:
/// what failed, at which pipeline stage, why (when known), and what the user
/// can do about it.
#[derive(Debug, thiserror::Error)]
#[error("{}", self.render())]
pub struct CueError {
    stage: Option<PipelineStage>,
    summary: String,
    cause: Option<String>,
    remedy: Option<String>,
}

impl CueError {
    /// An error attributed to a specific pipeline stage.
    pub fn new(stage: PipelineStage, summary: impl Into<String>) -> Self {
        Self {
            stage: Some(stage),
            summary: summary.into(),
            cause: None,
            remedy: None,
        }
    }

    /// An error outside any pipeline stage (config parsing, CLI usage).
    pub fn general(summary: impl Into<String>) -> Self {
        Self {
            stage: None,
            summary: summary.into(),
            cause: None,
            remedy: None,
        }
    }

    /// Attach the underlying reason, when known.
    pub fn because(mut self, cause: impl Into<String>) -> Self {
        self.cause = Some(cause.into());
        self
    }

    /// Attach actionable advice for the user.
    pub fn remedy(mut self, remedy: impl Into<String>) -> Self {
        self.remedy = Some(remedy.into());
        self
    }

    pub fn stage(&self) -> Option<PipelineStage> {
        self.stage
    }

    /// Attribute an error to the stage that owns the operation while
    /// preserving its summary, cause, and remedy.
    pub fn at_stage(mut self, stage: PipelineStage) -> Self {
        self.stage = Some(stage);
        self
    }

    /// Return the allowlisted projection suitable for durable records.
    pub fn persistent_failure(&self) -> PersistentFailure {
        PersistentFailure {
            stage: self.stage,
            summary: self.summary.clone(),
            remedy: self.remedy.clone(),
        }
    }

    fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.summary);
        out.push('\n');
        if let Some(stage) = self.stage {
            out.push_str(&format!("\nStage: {stage}\n"));
        }
        if let Some(cause) = &self.cause {
            out.push_str(&format!("\nReason: {cause}\n"));
        }
        if let Some(remedy) = &self.remedy {
            out.push_str(&format!("\nTry:\n    {remedy}\n"));
        }
        out
    }
}

impl From<std::io::Error> for CueError {
    fn from(err: std::io::Error) -> Self {
        CueError::general("an I/O operation failed").because(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_contains_all_four_parts() {
        let err = CueError::new(PipelineStage::Transcribe, "transcription failed")
            .because("could not load model \"large-v3-turbo\"")
            .remedy("run `cue doctor` for information about the local transcription environment");

        let rendered = err.to_string();
        assert!(rendered.contains("transcription failed"), "{rendered}");
        assert!(rendered.contains("Stage: transcribe"), "{rendered}");
        assert!(
            rendered.contains("could not load model"),
            "missing cause: {rendered}"
        );
        assert!(
            rendered.contains("cue doctor"),
            "missing remedy: {rendered}"
        );
    }

    #[test]
    fn display_omits_absent_parts() {
        let err = CueError::general("bad configuration");
        let rendered = err.to_string();
        assert!(!rendered.contains("Stage:"), "{rendered}");
        assert!(!rendered.contains("Reason:"), "{rendered}");
        assert!(!rendered.contains("Try:"), "{rendered}");
    }

    #[test]
    fn general_errors_carry_no_stage() {
        assert_eq!(CueError::general("boom").stage(), None);
        assert_eq!(
            CueError::new(PipelineStage::Render, "boom").stage(),
            Some(PipelineStage::Render)
        );
        assert_eq!(
            CueError::general("boom")
                .at_stage(PipelineStage::Render)
                .stage(),
            Some(PipelineStage::Render)
        );
    }

    #[test]
    fn io_error_maps_into_general() {
        let err: CueError =
            std::io::Error::new(std::io::ErrorKind::NotFound, "no such file").into();
        assert_eq!(err.stage(), None);
        assert!(err.to_string().contains("no such file"));
    }

    #[test]
    fn io_error_can_be_attributed_to_stage() {
        let err = CueError::new(PipelineStage::Extract, "ffmpeg failed").because("exit status 1");
        assert_eq!(err.stage(), Some(PipelineStage::Extract));
    }

    #[test]
    fn persistent_failure_excludes_the_cause() {
        let failure = CueError::new(PipelineStage::Transcribe, "transcription failed")
            .because("secret-token and private transcript text")
            .remedy("check the transcription provider")
            .persistent_failure();

        let json = serde_json::to_string(&failure).unwrap();
        assert!(json.contains("transcription failed"), "{json}");
        assert!(json.contains("check the transcription provider"), "{json}");
        assert!(!json.contains("secret-token"), "{json}");
        assert!(!json.contains("private transcript text"), "{json}");
    }
}
