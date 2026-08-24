//! Content-addressed caching for pipeline stages.

pub mod content;
pub mod layout;

pub use content::{bytes_hash, file_hash};
pub use layout::{cache_dir, media_dir};
