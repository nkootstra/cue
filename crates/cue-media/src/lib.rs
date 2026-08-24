//! External tool detection and media inspection.

pub mod checks;
pub mod probe;
pub mod tools;

pub use checks::{check_environment, check_python, check_tool, Environment};
pub use tools::ToolReport;
use serde::Serialize;

/// Outcome of probing a single external tool.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ToolStatus {
    /// Found on PATH and executable; carries the version banner.
    Available {
        path: String,
        version: String,
    },
    /// Not found anywhere on PATH.
    Missing,
    /// Found but could not be executed or misbehaved.
    Error(String),
}
