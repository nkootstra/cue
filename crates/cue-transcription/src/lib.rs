//! Transcription behind a provider-independent trait.
//!
//! cue's pipeline depends on [`Transcriber`], never on faster-whisper
//! directly, so providers can be swapped without touching domain code.

pub mod env;
pub mod faster_whisper;
pub mod options;
pub mod provision;
pub mod worker;

use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use cue_core::{Result, Transcript};

pub use env::WorkerEnvironment;
pub use faster_whisper::FasterWhisperTranscriber;
pub use options::TranscriptionOptions;
pub use provision::{provision, ProvisionAction};

/// PATH lookup re-exported from cue-media for environment resolution.
pub(crate) fn find_binary_on_path(name: &str) -> Option<PathBuf> {
    cue_media::tools::find_on_path(name)
}

/// Last few non-empty lines of a subprocess stderr, for error causes.
pub(crate) fn stderr_tail(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(5);
    lines[start..].join("\n")
}

/// A provider that turns audio into a canonical transcript.
#[async_trait]
pub trait Transcriber: Send + Sync {
    /// Human-readable provider name, used in logs and cache keys.
    fn name(&self) -> &str;

    async fn transcribe(
        &self,
        input: &Path,
        options: &TranscriptionOptions,
    ) -> Result<Transcript>;
}
