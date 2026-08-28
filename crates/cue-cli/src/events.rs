//! Pipeline event flow.
//!
//! Stage logic emits [`PipelineEvent`]s through a channel; this module
//! renders them. Rendering is TTY-aware: interactive terminals get a
//! spinner that collapses to a completed line, piped output gets plain
//! lines so scripts stay parseable.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::PathBuf;

use cue_core::{PipelineEvent, PipelineStage};

#[derive(Debug, Clone, PartialEq)]
pub struct FilePipelineEvent {
    pub source: PathBuf,
    pub event: PipelineEvent,
}

#[derive(Clone)]
pub struct FileEvents {
    source: PathBuf,
    sender: tokio::sync::mpsc::UnboundedSender<FilePipelineEvent>,
}

impl FileEvents {
    pub fn new(
        source: PathBuf,
        sender: tokio::sync::mpsc::UnboundedSender<FilePipelineEvent>,
    ) -> Self {
        Self { source, sender }
    }

    pub fn send(&self, event: PipelineEvent) {
        let _ = self.sender.send(FilePipelineEvent {
            source: self.source.clone(),
            event,
        });
    }
}

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

pub fn render_file_event(event: &FilePipelineEvent, is_batch: bool) -> String {
    let rendered = render_event(&event.event);
    if !is_batch || rendered.is_empty() {
        return rendered;
    }
    let source = source_label(&event.source);
    format!("  [{source}] {}", rendered.trim_start())
}

/// The human-facing name shown while a stage runs.
pub fn stage_label(stage: PipelineStage) -> &'static str {
    match stage {
        PipelineStage::Inspect => "inspecting",
        PipelineStage::Extract => "extracting audio",
        PipelineStage::Transcribe => "transcribing",
        PipelineStage::Normalize => "normalizing",
        PipelineStage::Analyze => "analyzing",
        PipelineStage::Render => "writing outputs",
    }
}

fn progress_label(stage: PipelineStage, percent: u8) -> String {
    format!("{} — {percent}%", stage_label(stage))
}

/// Consume events and print them appropriately for the output medium.
pub async fn run_renderer(
    rx: tokio::sync::mpsc::UnboundedReceiver<FilePipelineEvent>,
    is_batch: bool,
) {
    let interactive = std::io::stdout().is_terminal();
    if interactive && is_batch {
        run_batch_renderer(rx).await;
    } else {
        run_linear_renderer(rx, interactive, is_batch).await;
    }
}

async fn run_linear_renderer(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<FilePipelineEvent>,
    interactive: bool,
    is_batch: bool,
) {
    let mut open_spinner: Option<indicatif::ProgressBar> = None;

    while let Some(file_event) = rx.recv().await {
        let event = &file_event.event;
        match (event, interactive) {
            // Interactive: replace the spinner line as stages change.
            (PipelineEvent::Started(stage), true) => {
                if let Some(spinner) = open_spinner.take() {
                    spinner.finish_and_clear();
                }
                open_spinner = Some(spinner(stage_label(*stage).to_string()));
            }
            (PipelineEvent::Progress { stage, percent }, true) => {
                if let Some(spinner) = &open_spinner {
                    spinner.set_message(progress_label(*stage, *percent));
                }
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
                    println!("{}", render_file_event(&file_event, is_batch));
                }
            }
            _ => {
                // Piped output: plain deterministic lines, no spinner noise.
                match &event {
                    PipelineEvent::Started(stage) => {
                        let line = format!("  [{}] {}", short_stage(*stage), stage_label(*stage));
                        println!("{}", prefix_source(&line, &file_event.source, is_batch));
                    }
                    PipelineEvent::Cached(stage) => {
                        let line = format!(
                            "  [{}] {} (cached)",
                            short_stage(*stage),
                            stage_label(*stage)
                        );
                        println!("{}", prefix_source(&line, &file_event.source, is_batch));
                    }
                    PipelineEvent::Failed { stage, error } => {
                        let line = format!(
                            "  [{}] {} failed: {error}",
                            short_stage(*stage),
                            stage_label(*stage)
                        );
                        println!("{}", prefix_source(&line, &file_event.source, is_batch));
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

fn prefix_source(line: &str, source: &std::path::Path, is_batch: bool) -> String {
    if !is_batch {
        return line.to_owned();
    }
    let source = source_label(source);
    format!("  [{source}] {}", line.trim_start())
}

async fn run_batch_renderer(mut rx: tokio::sync::mpsc::UnboundedReceiver<FilePipelineEvent>) {
    let progress = indicatif::MultiProgress::new();
    let mut spinners = HashMap::new();

    while let Some(file_event) = rx.recv().await {
        let source = &file_event.source;
        match &file_event.event {
            PipelineEvent::Started(stage) => {
                finish_spinner(&mut spinners, source);
                let spinner = progress.add(spinner(batch_label(source, *stage, None)));
                spinners.insert(source.clone(), spinner);
            }
            PipelineEvent::Progress { stage, percent } => {
                if let Some(spinner) = spinners.get(source) {
                    spinner.set_message(batch_label(source, *stage, Some(*percent)));
                }
            }
            PipelineEvent::Completed(_) => finish_spinner(&mut spinners, source),
            PipelineEvent::Cached(_) | PipelineEvent::Failed { .. } => {
                finish_spinner(&mut spinners, source);
                let _ = progress.println(render_file_event(&file_event, true));
            }
        }
    }

    for (_, spinner) in spinners {
        spinner.finish_and_clear();
    }
}

fn spinner(message: String) -> indicatif::ProgressBar {
    let spinner = indicatif::ProgressBar::new_spinner();
    spinner.set_style(
        indicatif::ProgressStyle::default_spinner()
            .template("{spinner} {msg}")
            .expect("static template"),
    );
    spinner.set_message(message);
    spinner.enable_steady_tick(std::time::Duration::from_millis(120));
    spinner
}

fn finish_spinner(
    spinners: &mut HashMap<PathBuf, indicatif::ProgressBar>,
    source: &std::path::Path,
) {
    if let Some(spinner) = spinners.remove(source) {
        spinner.finish_and_clear();
    }
}

fn batch_label(source: &std::path::Path, stage: PipelineStage, percent: Option<u8>) -> String {
    let source = source_label(source);
    match percent {
        Some(percent) => format!("{source}: {}", progress_label(stage, percent)),
        None => format!("{source}: {}", stage_label(stage)),
    }
}

fn source_label(source: &std::path::Path) -> std::borrow::Cow<'_, str> {
    source.to_string_lossy()
}

fn short_stage(stage: PipelineStage) -> &'static str {
    stage.name()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
                percent: 1,
            }),
            ""
        );
    }

    #[test]
    fn progress_label_uses_the_stage_carried_by_the_event() {
        assert_eq!(
            progress_label(PipelineStage::Analyze, 40),
            "analyzing — 40%"
        );
        assert_eq!(
            progress_label(PipelineStage::Transcribe, 40),
            "transcribing — 40%"
        );
    }

    #[test]
    fn batch_event_lines_identify_the_source_file() {
        let event = FilePipelineEvent {
            source: PathBuf::from("course/lesson-one.mp4"),
            event: PipelineEvent::Started(PipelineStage::Transcribe),
        };

        assert_eq!(
            render_file_event(&event, true),
            "  [course/lesson-one.mp4] [transcribe] running"
        );
        assert_eq!(render_file_event(&event, false), "  [transcribe] running");
    }

    #[test]
    fn file_event_sender_preserves_source_identity() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let events = FileEvents::new(PathBuf::from("course/lesson.mp4"), tx);

        events.send(PipelineEvent::Cached(PipelineStage::Extract));

        assert_eq!(
            rx.try_recv().unwrap(),
            FilePipelineEvent {
                source: PathBuf::from("course/lesson.mp4"),
                event: PipelineEvent::Cached(PipelineStage::Extract),
            }
        );
    }
}
