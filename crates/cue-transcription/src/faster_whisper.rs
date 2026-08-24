//! The faster-whisper adapter: launches the Python worker and maps its
//! output onto the canonical transcript.

use std::path::Path;

use async_trait::async_trait;
use cue_core::{CueError, PipelineStage, Result, Transcript};
use tracing::{debug, instrument};

use crate::Transcriber;
use crate::env::WorkerEnvironment;
use crate::options::TranscriptionOptions;
use crate::worker::WorkerOutput;

/// Runs transcription through the embedded worker script.
pub struct FasterWhisperTranscriber {
    env: WorkerEnvironment,
}

impl FasterWhisperTranscriber {
    pub fn new(env: WorkerEnvironment) -> Self {
        Self { env }
    }

    /// Build with automatic environment resolution (venv or system python).
    pub fn resolve(python_override: Option<&Path>) -> cue_core::Result<Self> {
        Ok(Self::new(crate::env::resolve(python_override)?))
    }

    fn spawn_error(&self, err: std::io::Error) -> CueError {
        CueError::new(
            PipelineStage::Transcribe,
            "could not launch the faster-whisper worker",
        )
        .because(err.to_string())
        .remedy("run `cue doctor` to check the local transcription environment")
    }

    fn failure(&self, stderr: &str) -> CueError {
        let tail = crate::stderr_tail(stderr);
        CueError::new(
            PipelineStage::Transcribe,
            "the faster-whisper worker failed",
        )
        .because(tail)
        .remedy(
            "check that the model name is valid and `cue doctor` reports a \
             working environment",
        )
    }
}

#[async_trait]
impl Transcriber for FasterWhisperTranscriber {
    fn name(&self) -> &str {
        "faster-whisper"
    }

    #[instrument(skip(self, input, options), fields(model = %options.model))]
    async fn transcribe(&self, input: &Path, options: &TranscriptionOptions) -> Result<Transcript> {
        self.transcribe_with_progress(input, options, None).await
    }

    #[instrument(skip(self, input, options, progress), fields(model = %options.model))]
    async fn transcribe_with_progress(
        &self,
        input: &Path,
        options: &TranscriptionOptions,
        progress: Option<tokio::sync::mpsc::UnboundedSender<cue_core::PipelineEvent>>,
    ) -> Result<Transcript> {
        debug!(worker = %self.env.script.display(), "launching worker");

        let mut command = tokio::process::Command::new(&self.env.python);
        command
            .arg(&self.env.script)
            .arg("--input")
            .arg(input)
            .arg("--model")
            .arg(&options.model)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(language) = &options.language {
            command.arg("--language").arg(language);
        }

        let mut child = command.spawn().map_err(|e| self.spawn_error(e))?;

        // Drain stderr concurrently: PROGRESS markers become pipeline
        // events, everything else is kept for error tails and debug logs.
        let stderr = child.stderr.take().expect("stderr was piped");
        let stderr_task = tokio::spawn(drain_stderr(stderr, progress));

        let stdout = child.stdout.take().expect("stdout was piped");
        let stdout_task = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = Vec::new();
            let mut stdout = stdout;
            stdout
                .read_to_end(&mut buf)
                .await
                .map(|_| buf)
                .map_err(|e| e.to_string())
        });

        let status = child.wait().await.map_err(|e| self.spawn_error(e))?;
        let stdout_bytes = stdout_task
            .await
            .map_err(|e| CueError::general("worker stdout reader failed").because(e.to_string()))?
            .map_err(|e| CueError::general("could not read worker stdout").because(e))?;
        let stderr_text = stderr_task.await.unwrap_or_default();

        if !status.success() {
            return Err(self.failure(&stderr_text));
        }

        let parsed: WorkerOutput = serde_json::from_slice(&stdout_bytes).map_err(|e| {
            CueError::new(
                PipelineStage::Transcribe,
                "the faster-whisper worker produced unreadable output",
            )
            .because(format!(
                "{e}; stderr tail: {}",
                crate::stderr_tail(&String::from_utf8_lossy(&stdout_bytes))
            ))
        })?;

        Ok(parsed.into_transcript())
    }
}

/// Read worker stderr to completion, forwarding `PROGRESS <fraction>` lines
/// as pipeline events and returning the full text for error tails.
async fn drain_stderr(
    stderr: impl tokio::io::AsyncRead + Unpin,
    progress: Option<tokio::sync::mpsc::UnboundedSender<cue_core::PipelineEvent>>,
) -> String {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut collected = String::new();
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        collected.push_str(&line);
        collected.push('\n');
        if let Some(rest) = line.strip_prefix("PROGRESS ")
            && let Ok(fraction) = rest.trim().parse::<f32>()
            && let Some(sender) = &progress
        {
            let percent = (fraction.clamp(0.0, 1.0) * 100.0).round() as u64;
            let _ = sender.send(cue_core::PipelineEvent::Progress {
                stage: PipelineStage::Transcribe,
                current: percent,
                total: Some(100),
            });
        } else if !line.trim().is_empty() {
            debug!(target: "cue_worker", "{line}");
        }
    }
    collected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::TranscriptionOptions;

    /// A fake worker script exercising the adapter contract without any
    /// model inference.
    const FAKE_OK_SCRIPT: &str = r#"
import json, sys
sys.stdout.write(json.dumps({
    "version": 1,
    "language": "en",
    "duration": 4.2,
    "segments": [
        {"id": 0, "start": 0.0, "end": 0.6, "text": " hello world,",
         "words": [
             {"word": " hello", "start": 0.0, "end": 0.3, "probability": 0.97},
             {"word": " world,", "start": 0.31, "end": 0.6, "probability": 0.91}
         ]}
    ]
}))
"#;

    const FAKE_FAIL_SCRIPT: &str = r#"
import sys
sys.stderr.write("loading model large-v3-turbo\n")
sys.stderr.write("error: model not found on this system\n")
sys.exit(3)
"#;

    fn write_script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn python() -> std::path::PathBuf {
        // Tests run where python3 exists; skip gracefully otherwise.
        match crate::find_binary_on_path("python3") {
            Some(p) => p,
            None => panic!("python3 required for adapter tests"),
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cue-transcribe-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_input(dir: &std::path::Path) -> std::path::PathBuf {
        let wav = dir.join("audio.wav");
        std::fs::write(&wav, b"RIFF....").unwrap();
        wav
    }

    #[tokio::test]
    async fn adapter_maps_worker_output_to_transcript() {
        let dir = temp_dir("ok");
        let script = write_script(&dir, "fake_ok.py", FAKE_OK_SCRIPT);
        let transcriber = FasterWhisperTranscriber::new(WorkerEnvironment {
            python: python(),
            script,
        });

        let transcript = transcriber
            .transcribe(&fake_input(&dir), &TranscriptionOptions::default())
            .await
            .unwrap();

        assert_eq!(transcript.language, "en");
        assert_eq!(transcript.words.len(), 2);
        assert_eq!(transcript.words[0].text, "hello");
        assert_eq!(transcript.segments[0].word_end, 2);
    }

    #[tokio::test]
    async fn adapter_surfaces_stderr_tail_as_cause() {
        let dir = temp_dir("fail");
        let script = write_script(&dir, "fake_fail.py", FAKE_FAIL_SCRIPT);
        let transcriber = FasterWhisperTranscriber::new(WorkerEnvironment {
            python: python(),
            script,
        });

        let err = transcriber
            .transcribe(&fake_input(&dir), &TranscriptionOptions::default())
            .await
            .unwrap_err();

        assert_eq!(err.stage(), Some(PipelineStage::Transcribe));
        let rendered = err.to_string();
        assert!(rendered.contains("model not found"), "{rendered}");
        assert!(rendered.contains("cue doctor"), "{rendered}");
    }

    const FAKE_PROGRESS_SCRIPT: &str = r#"
import json, sys
for pct in ("0.25", "0.5", "0.75"):
    sys.stderr.write(f"PROGRESS {pct}\n")
    sys.stderr.flush()
sys.stdout.write(json.dumps({
    "version": 1, "language": "en", "duration": 2.0,
    "segments": [{"id": 0, "start": 0.0, "end": 2.0, "text": " done.",
                  "words": [{"word": " done.", "start": 0.0, "end": 2.0,
                             "probability": 0.9}]}]
}))
"#;

    #[tokio::test]
    async fn progress_markers_become_pipeline_events() {
        let dir = temp_dir("progress");
        let script = write_script(&dir, "fake_progress.py", FAKE_PROGRESS_SCRIPT);
        let transcriber = FasterWhisperTranscriber::new(WorkerEnvironment {
            python: python(),
            script,
        });

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let transcript = transcriber
            .transcribe_with_progress(
                &fake_input(&dir),
                &TranscriptionOptions::default(),
                Some(tx),
            )
            .await
            .unwrap();
        assert_eq!(transcript.words.len(), 1);

        let mut percents = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let cue_core::PipelineEvent::Progress { current, total, .. } = event {
                assert_eq!(total, Some(100));
                percents.push(current);
            }
        }
        assert_eq!(percents, vec![25, 50, 75]);
    }

    #[tokio::test]
    async fn plain_transcribe_ignores_progress_markers() {
        let dir = temp_dir("noprogress");
        let script = write_script(&dir, "fake_progress2.py", FAKE_PROGRESS_SCRIPT);
        let transcriber = FasterWhisperTranscriber::new(WorkerEnvironment {
            python: python(),
            script,
        });

        // PROGRESS lines on stderr are simply ignored.
        let transcript = transcriber
            .transcribe(&fake_input(&dir), &TranscriptionOptions::default())
            .await
            .unwrap();
        assert_eq!(transcript.language, "en");
    }

    #[test]
    fn stderr_tail_keeps_last_lines_only() {
        let tail = crate::stderr_tail("line1\nline2\nline3\nline4\nline5\nline6\nline7");
        assert!(!tail.contains("line1"));
        assert!(!tail.contains("line2"));
        assert!(tail.contains("line7"));
    }
}
