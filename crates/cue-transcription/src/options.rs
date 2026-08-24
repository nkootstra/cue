//! Options controlling a transcription run.
//!
//! These feed both the provider and the cache key: changing any of them may
//! change the output, so they must be part of the transcription identity.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionOptions {
    /// Model name understood by the provider ("large-v3-turbo").
    pub model: String,
    /// Language hint; `None` lets the transcriber auto-detect.
    pub language: Option<String>,
}

impl Default for TranscriptionOptions {
    fn default() -> Self {
        Self {
            model: "large-v3-turbo".into(),
            language: None,
        }
    }
}
