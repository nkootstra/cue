//! LLM gateway and Ollama clients shared by normalization and analysis.

pub mod chat;
pub mod ollama;

pub use chat::{ChatClient, ChatMessage, ChatResponse};
pub use ollama::{OllamaAdmin, OllamaModel};
