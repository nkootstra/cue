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

const STDERR_CAPTURE_LIMIT: usize = 16 * 1024;
const STDERR_LINE_LIMIT: usize = 4 * 1024;

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
                "{e}; worker stderr tail: {}",
                crate::stderr_tail(&stderr_text)
            ))
            .remedy(
                "run `cue doctor`; if CUE_FASTER_WHISPER_SCRIPT is set, update or remove that override",
            )
        })?;

        parsed.into_transcript()
    }
}

/// Read worker stderr to completion, forwarding `PROGRESS <fraction>` lines
/// as pipeline events and returning a bounded tail for diagnostics.
async fn drain_stderr(
    stderr: impl tokio::io::AsyncRead + Unpin,
    progress: Option<tokio::sync::mpsc::UnboundedSender<cue_core::PipelineEvent>>,
) -> String {
    use std::collections::VecDeque;
    use tokio::io::AsyncReadExt;

    let mut stderr = stderr;
    let mut tail = VecDeque::with_capacity(STDERR_CAPTURE_LIMIT);
    let mut line = Vec::with_capacity(256);
    let mut line_truncated = false;
    let mut chunk = [0_u8; 4096];

    loop {
        let read = match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };

        for &byte in &chunk[..read] {
            if tail.len() == STDERR_CAPTURE_LIMIT {
                tail.pop_front();
            }
            tail.push_back(byte);

            if byte == b'\n' {
                process_stderr_line(&line, line_truncated, progress.as_ref());
                line.clear();
                line_truncated = false;
            } else if line.len() < STDERR_LINE_LIMIT {
                line.push(byte);
            } else {
                line_truncated = true;
            }
        }
    }

    if !line.is_empty() || line_truncated {
        process_stderr_line(&line, line_truncated, progress.as_ref());
    }

    String::from_utf8_lossy(tail.make_contiguous()).into_owned()
}

fn process_stderr_line(
    bytes: &[u8],
    truncated: bool,
    progress: Option<&tokio::sync::mpsc::UnboundedSender<cue_core::PipelineEvent>>,
) {
    if truncated {
        debug!(target: "cue_worker", "worker stderr line exceeded capture limit");
        return;
    }

    let line = String::from_utf8_lossy(bytes);
    let line = line.trim_end_matches('\r');
    if let Some(rest) = line.strip_prefix("PROGRESS ")
        && let Ok(fraction) = rest.trim().parse::<f32>()
        && let Some(sender) = progress
    {
        let percent = (fraction.clamp(0.0, 1.0) * 100.0).round() as u8;
        let _ = sender.send(cue_core::PipelineEvent::Progress {
            stage: PipelineStage::Transcribe,
            percent,
        });
    } else if !line.trim().is_empty() {
        debug!(target: "cue_worker", "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::TranscriptionOptions;

    const REAL_WORKER_SCRIPT: &str =
        include_str!("../../../workers/faster-whisper/src/cue_faster_whisper.py");

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

    fn write_fake_backend(dir: &std::path::Path, body: &str) {
        std::fs::write(dir.join("faster_whisper.py"), body).unwrap();
    }

    fn real_worker(dir: &std::path::Path) -> FasterWhisperTranscriber {
        let script = write_script(dir, "cue_faster_whisper.py", REAL_WORKER_SCRIPT);
        FasterWhisperTranscriber::new(WorkerEnvironment {
            python: python(),
            script,
        })
    }

    const FAKE_BACKEND: &str = r#"
from types import SimpleNamespace

__version__ = "test"

class WhisperModel:
    def __init__(self, model_name, **kwargs):
        self.model_name = model_name
        if model_name == "load-error":
            raise RuntimeError("model load exploded")

    def transcribe(self, input_path, **kwargs):
        info = SimpleNamespace(language="en", duration=2.0)

        def segments():
            yield SimpleNamespace(
                start=0.0,
                end=1.0,
                text=" hello",
                words=[SimpleNamespace(word=" hello", start=0.0, end=1.0, probability=0.9)],
            )
            if self.model_name == "lazy-error":
                raise RuntimeError("lazy iterator exploded")

        return segments(), info
"#;

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

    #[tokio::test]
    async fn embedded_worker_and_fake_backend_satisfy_v1_contract() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_backend(dir.path(), FAKE_BACKEND);
        let transcriber = real_worker(dir.path());

        let transcript = transcriber
            .transcribe(&fake_input(dir.path()), &TranscriptionOptions::default())
            .await
            .unwrap();

        assert_eq!(transcript.language, "en");
        assert_eq!(transcript.words[0].text, "hello");
        assert_eq!(transcript.segments[0].word_end, 1);
    }

    #[tokio::test]
    async fn embedded_worker_reports_import_load_and_lazy_iterator_failures() {
        let cases = [
            (
                "import",
                "raise ImportError('backend import exploded')\n",
                "large-v3-turbo",
                "backend import exploded",
            ),
            ("load", FAKE_BACKEND, "load-error", "model load exploded"),
            ("lazy", FAKE_BACKEND, "lazy-error", "lazy iterator exploded"),
        ];

        for (name, backend, model, expected) in cases {
            let dir = tempfile::tempdir().unwrap();
            write_fake_backend(dir.path(), backend);
            let transcriber = real_worker(dir.path());
            let options = TranscriptionOptions {
                model: model.to_string(),
                ..TranscriptionOptions::default()
            };

            let rendered = transcriber
                .transcribe(&fake_input(dir.path()), &options)
                .await
                .unwrap_err()
                .to_string();

            assert!(rendered.contains(expected), "{name}: {rendered}");
            assert!(
                rendered.contains("cue-faster-whisper:"),
                "{name}: {rendered}"
            );
            assert!(!rendered.contains("Traceback"), "{name}: {rendered}");
        }
    }

    #[tokio::test]
    async fn embedded_worker_rejects_missing_input_without_stdout_json() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_backend(dir.path(), FAKE_BACKEND);
        let script = write_script(dir.path(), "cue_faster_whisper.py", REAL_WORKER_SCRIPT);

        let output = tokio::process::Command::new(python())
            .arg(script)
            .output()
            .await
            .unwrap();

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("cue-faster-whisper: invalid arguments"));
        assert!(stderr.contains("--input is required"));
    }

    #[tokio::test]
    async fn adapter_rejects_missing_and_unsupported_protocol_versions() {
        for (version_field, expected) in [
            ("", "missing field `version`"),
            (r#""version": 0,"#, "protocol version 0"),
            (r#""version": 2,"#, "protocol version 2"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let body = format!(
                r#"import json
print(json.dumps({{{version_field} "language": "en", "segments": []}}))
"#
            );
            let script = write_script(dir.path(), "override.py", &body);
            let transcriber = FasterWhisperTranscriber::new(WorkerEnvironment {
                python: python(),
                script,
            });

            let rendered = transcriber
                .transcribe(&fake_input(dir.path()), &TranscriptionOptions::default())
                .await
                .unwrap_err()
                .to_string();

            assert!(rendered.contains(expected), "{rendered}");
            assert!(rendered.contains("cue doctor"), "{rendered}");
            assert!(rendered.contains("CUE_FASTER_WHISPER_SCRIPT"), "{rendered}");
        }
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
            if let cue_core::PipelineEvent::Progress { percent, .. } = event {
                percents.push(percent);
            }
        }
        assert_eq!(percents, vec![25, 50, 75]);
    }

    #[test]
    fn progress_markers_are_clamped_to_valid_percentages() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        process_stderr_line(b"PROGRESS -0.5", false, Some(&tx));
        process_stderr_line(b"PROGRESS 1.5", false, Some(&tx));

        let percents = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|event| match event {
                cue_core::PipelineEvent::Progress { percent, .. } => percent,
                other => panic!("unexpected event: {other:?}"),
            })
            .collect::<Vec<_>>();

        assert_eq!(percents, [0, 100]);
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

    #[tokio::test]
    async fn stderr_capture_is_bounded_and_keeps_useful_tail() {
        use tokio::io::AsyncWriteExt;

        let (mut writer, reader) = tokio::io::duplex(1024);
        let writer_task = tokio::spawn(async move {
            writer
                .write_all(&vec![b'x'; STDERR_CAPTURE_LIMIT * 2])
                .await
                .unwrap();
            writer
                .write_all(b"\ncue-faster-whisper: final failure\n")
                .await
                .unwrap();
        });

        let captured = drain_stderr(reader, None).await;
        writer_task.await.unwrap();

        assert!(captured.len() <= STDERR_CAPTURE_LIMIT);
        assert!(captured.ends_with("cue-faster-whisper: final failure\n"));
    }
}
