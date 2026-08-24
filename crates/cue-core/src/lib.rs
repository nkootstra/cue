pub mod config;
pub mod error;
pub mod media;
pub mod normalized;
pub mod pipeline;
pub mod transcript;

pub use config::Config;
pub use error::CueError;
pub use media::{AudioStream, Media, VideoStream};
pub use normalized::{
    NormalizedChunk, NormalizedTranscript, NORMALIZED_SCHEMA_VERSION,
};
pub use pipeline::{PipelineEvent, PipelineStage};
pub use transcript::{Segment, Transcript, Word, TRANSCRIPT_SCHEMA_VERSION};

pub type Result<T, E = CueError> = std::result::Result<T, E>;
