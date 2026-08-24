pub mod config;
pub mod error;
pub mod pipeline;

pub use config::Config;
pub use error::CueError;
pub use pipeline::{PipelineEvent, PipelineStage};

pub type Result<T, E = CueError> = std::result::Result<T, E>;
