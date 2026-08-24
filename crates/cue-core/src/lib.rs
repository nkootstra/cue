pub mod analysis;
pub mod config;
pub mod error;
pub mod media;
pub mod normalized;
pub mod pipeline;
pub mod transcript;

pub use analysis::{ANALYSIS_SCHEMA_VERSION, Analysis, Topic};
pub use config::Config;
pub use error::CueError;
pub use media::{AudioStream, Media, VideoStream};
pub use normalized::{NORMALIZED_SCHEMA_VERSION, NormalizedChunk, NormalizedTranscript};
pub use pipeline::{PipelineEvent, PipelineStage};
pub use transcript::{Segment, TRANSCRIPT_SCHEMA_VERSION, Transcript, Word};

pub type Result<T, E = CueError> = std::result::Result<T, E>;
