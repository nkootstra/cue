//! Pipeline event flow.
//!
//! Stage logic emits [`PipelineEvent`]s through a channel; this module
//! renders them. Rendering is TTY-aware: interactive terminals get a
//! spinner that collapses to a completed line, piped output gets plain
//! lines so scripts stay parseable.

use std::io::IsTerminal;

use cue_core::{PipelineEvent, PipelineStage};

/// One rendered line for an event, shared by both TTY and pipe renderers.
///
/// Public so tests can assert on exact wording.
pub fn render_event(event: &PipelineEvent) -> String {
    match event {
        PipelineEvent::Started(stage) => format!("  [{}] running", stage),
        PipelineEvent::Completed(stage) => format!("  [{}] done", stage),
        PipelineEvent::Cached(stage) => format!("  [{}] cached", stage),
        PipelineEvent::Failed { stage, error } => format!("  [{}] failed: {error}", stage),
        PipelineEvent::Progress { .. } => String::new(),
    }
}

/// The human-facing name shown while a stage runs.
pub fn stage_label(stage: PipelineStage) -> &'static str {
    match stage {
        PipelineStage::Inspect => "inspecting",
        PipelineStage::Extract => "extracting audio",
        PipelineStage::Transcribe => "transcribing",
        PipelineStage::Caption => "building subtitles",
        PipelineStage::Normalize => "normalizing",
        PipelineStage::Analyze => "analyzing",
        PipelineStage::Render => "writing outputs",
    }
}

/// Consume events and print them appropriately for the output medium.
pub async fn run_renderer(mut rx: tokio::sync::mpsc::UnboundedReceiver<PipelineEvent>) {
    let interactive = std::io::stdout().is_terminal();
    let mut open_spinner: Option<indicatif::ProgressBar> = None;

    while let Some(event) = rx.recv().await {
        match (&event, interactive) {
            // Interactive: replace the spinner line as stages change.
            (PipelineEvent::Started(stage), true) => {
                if let Some(spinner) = open_spinner.take() {
                    spinner.finish_and_clear();
                }
                let spinner = indicatif::ProgressBar::new_spinner();
                spinner.set_style(
                    indicatif::ProgressStyle::default_spinner()
                        .template("{spinner} {msg}")
                        .expect("static template"),
                );
                spinner.set_message(stage_label(*stage).to_string());
                spinner.enable_steady_tick(std::time::Duration::from_millis(120));
                open_spinner = Some(spinner);
            }
            (PipelineEvent::Cached(_), true)
            | (PipelineEvent::Completed(_), true)
            | (PipelineEvent::Failed { .. }, true) => {
                if let Some(spinner) = open_spinner.take() {
                    spinner.finish_and_clear();
                }
                if !matches!(event, PipelineEvent::Completed(_)) {
                    // Completed stages are silent under a spinner; cached and
                    // failed states still say something.
                    println!("{}", render_event(&event));
                }
            }
            _ => {
                // Piped output: plain deterministic lines, no spinner noise.
                match &event {
                    PipelineEvent::Started(stage) => {
                        println!("  [{}] {}", short_stage(*stage), stage_label(*stage));
                    }
                    PipelineEvent::Cached(stage) => {
                        println!(
                            "  [{}] {} (cached)",
                            short_stage(*stage),
                            stage_label(*stage)
                        );
                    }
                    PipelineEvent::Failed { stage, error } => {
                        println!(
                            "  [{}] {} failed: {error}",
                            short_stage(*stage),
                            stage_label(*stage)
                        );
                    }
                    PipelineEvent::Completed(_) | PipelineEvent::Progress { .. } => {}
                }
            }
        }
    }

    if let Some(spinner) = open_spinner.take() {
        spinner.finish_and_clear();
    }
}

fn short_stage(stage: PipelineStage) -> &'static str {
    stage.name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_render_deterministically() {
        let started = render_event(&PipelineEvent::Started(PipelineStage::Extract));
        assert_eq!(started, "  [extract] running");

        let cached = render_event(&PipelineEvent::Cached(PipelineStage::Normalize));
        assert_eq!(cached, "  [normalize] cached");

        let failed = render_event(&PipelineEvent::Failed {
            stage: PipelineStage::Analyze,
            error: "gateway unreachable".into(),
        });
        assert!(failed.contains("[analyze] failed"), "{failed}");
        assert!(failed.contains("gateway unreachable"), "{failed}");
    }

    #[test]
    fn every_stage_has_a_human_label() {
        for stage in PipelineStage::ALL {
            assert!(!stage_label(stage).is_empty(), "{stage} lacks a label");
        }
    }

    #[test]
    fn progress_events_render_empty() {
        assert_eq!(
            render_event(&PipelineEvent::Progress {
                stage: PipelineStage::Transcribe,
                current: 1,
                total: None,
            }),
            ""
        );
    }
}
