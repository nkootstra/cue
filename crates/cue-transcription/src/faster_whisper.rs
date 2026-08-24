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

        let output = command.output().await.map_err(|e| self.spawn_error(e))?;

        if !output.status.success() {
            return Err(self.failure(&String::from_utf8_lossy(&output.stderr)));
        }

        let parsed: WorkerOutput = serde_json::from_slice(&output.stdout).map_err(|e| {
            CueError::new(
                PipelineStage::Transcribe,
                "the faster-whisper worker produced unreadable output",
            )
            .because(format!(
                "{e}; stderr tail: {}",
                crate::stderr_tail(&String::from_utf8_lossy(&output.stderr))
            ))
        })?;

        Ok(parsed.into_transcript())
    }
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

    #[test]
    fn stderr_tail_keeps_last_lines_only() {
        let tail = crate::stderr_tail("line1\nline2\nline3\nline4\nline5\nline6\nline7");
        assert!(!tail.contains("line1"));
        assert!(!tail.contains("line2"));
        assert!(tail.contains("line7"));
    }
}
