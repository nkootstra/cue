pub mod config;
pub mod error;
pub mod media;
pub mod pipeline;

pub use config::Config;
pub use error::CueError;
pub use media::{AudioStream, Media, VideoStream};
pub use pipeline::{PipelineEvent, PipelineStage};

pub type Result<T, E = CueError> = std::result::Result<T, E>;
