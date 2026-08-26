//! Ollama-native chat and model administration.
//!
//! The chat client provides non-streaming inference with thinking disabled.
//! The administrative operations list, pull, and create models for
//! `cue models list/check/install`.

use serde::{Deserialize, Serialize};

use cue_core::{CueError, Result};

use crate::{ChatMessage, ChatResponse};

/// Client for Ollama's native non-streaming chat API.
///
/// Ollama's OpenAI-compatible endpoint may move text produced by thinking
/// models into a non-standard `reasoning` field. The native endpoint lets
/// callers disable thinking explicitly and keeps the final text in
/// `message.content`.
#[derive(Debug, Clone)]
pub struct OllamaChatClient {
    http: reqwest::Client,
    base_url: String,
}

impl OllamaChatClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    pub async fn chat(
        &self,
        model: &str,
        messages: &[ChatMessage],
        temperature: Option<f32>,
    ) -> Result<ChatResponse> {
        #[derive(Serialize)]
        struct ChatOptions {
            #[serde(skip_serializing_if = "Option::is_none")]
            temperature: Option<f32>,
        }

        #[derive(Serialize)]
        struct ChatRequest<'a> {
            model: &'a str,
            messages: &'a [ChatMessage],
            stream: bool,
            think: bool,
            options: ChatOptions,
        }

        let response = self
            .http
            .post(format!("{}/api/chat", self.base_url))
            .json(&ChatRequest {
                model,
                messages,
                stream: false,
                think: false,
                options: ChatOptions { temperature },
            })
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| ollama_unreachable(e.to_string()))?;

        if !response.status().is_success() {
            return Err(http_error("running", model, response.status()));
        }

        let response: NativeChatResponse = response
            .json()
            .await
            .map_err(|e| ollama_error("Ollama returned unreadable chat output", e.to_string()))?;

        Ok(ChatResponse {
            content: response.message.content,
            prompt_tokens: response.prompt_eval_count,
            completion_tokens: response.eval_count,
        })
    }
}

/// Client for Ollama's native API (port 11434 root, no `/v1`).
#[derive(Debug, Clone)]
pub struct OllamaAdmin {
    http: reqwest::Client,
    base_url: String,
}

impl OllamaAdmin {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// The server base URL this client talks to (e.g.
    /// `http://localhost:11434`), without the trailing slash.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Names of models already present locally.
    pub async fn list_models(&self) -> Result<Vec<OllamaModel>> {
        let response = self
            .http
            .get(format!("{}/api/tags", self.base_url))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| ollama_unreachable(e.to_string()))?;
        if !response.status().is_success() {
            return Err(http_error("listing", "models", response.status()));
        }
        let response: TagsResponse = response
            .json()
            .await
            .map_err(|e| ollama_error("Ollama returned unreadable output", e.to_string()))?;

        Ok(response
            .models
            .into_iter()
            .map(|m| OllamaModel { name: m.name })
            .collect())
    }

    /// True when a model with this name exists, treating the default
    /// `:latest` tag as equivalent (`cue-s1-mini` matches
    /// `cue-s1-mini:latest`).
    pub async fn has_model(&self, name: &str) -> Result<bool> {
        Ok(Self::models_include(&self.list_models().await?, name))
    }

    /// Test a model-list snapshot without issuing another `/api/tags`
    /// request.
    pub fn models_include(models: &[OllamaModel], name: &str) -> bool {
        models
            .iter()
            .any(|model| model_name_matches(&model.name, name))
    }

    /// Pull a model by reference (e.g. an `hf.co/...` GGUF tag).
    pub async fn pull(&self, model_ref: &str) -> Result<()> {
        #[derive(Serialize)]
        struct PullRequest<'a> {
            model: &'a str,
            stream: bool,
        }

        let response = self
            .http
            .post(format!("{}/api/pull", self.base_url))
            .json(&PullRequest {
                model: model_ref,
                stream: false,
            })
            .timeout(std::time::Duration::from_secs(3600))
            .send()
            .await
            .map_err(|e| ollama_unreachable(e.to_string()))?;

        if !response.status().is_success() {
            return Err(http_error("pulling", model_ref, response.status()));
        }
        Ok(())
    }

    /// Create a named model from Modelfile text.
    ///
    /// Note: current Ollama releases reject Modelfiles whose `FROM` names a
    /// registry ref (e.g. `hf.co/...`) through this API. `cue models install`
    /// uses the `ollama` CLI instead; this method remains for plain local
    /// Modelfiles.
    pub async fn create(&self, name: &str, modelfile: &str) -> Result<()> {
        #[derive(Serialize)]
        struct CreateRequest<'a> {
            model: &'a str,
            modelfile: &'a str,
            stream: bool,
        }

        let response = self
            .http
            .post(format!("{}/api/create", self.base_url))
            .json(&CreateRequest {
                model: name,
                modelfile,
                stream: false,
            })
            .timeout(std::time::Duration::from_secs(600))
            .send()
            .await
            .map_err(|e| ollama_unreachable(e.to_string()))?;

        if !response.status().is_success() {
            return Err(http_error("creating model", name, response.status()));
        }
        Ok(())
    }
}

/// Build an actionable status-only error. Provider bodies can contain
/// request data, so they are deliberately neither read nor logged.
fn http_error(verb: &str, subject: &str, status: reqwest::StatusCode) -> CueError {
    CueError::new(
        cue_core::PipelineStage::Normalize,
        format!("{verb} {subject} failed ({status})"),
    )
    .remedy("run `cue doctor` to inspect the local Ollama setup, or use the `ollama` CLI directly")
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OllamaModel {
    pub name: String,
}

fn ollama_unreachable(reason: String) -> CueError {
    CueError::new(cue_core::PipelineStage::Normalize, "could not reach Ollama")
        .because(reason)
        .remedy("is Ollama running? start it or set normalization.ollama_url in cue.toml")
}

fn ollama_error(summary: impl Into<String>, reason: String) -> CueError {
    CueError::new(cue_core::PipelineStage::Normalize, summary).because(reason)
}

#[derive(Debug, Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagsModel>,
}

#[derive(Debug, Deserialize)]
struct NativeChatResponse {
    message: ChatMessage,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TagsModel {
    name: String,
}

/// `name == target`, or `name == "{target}:latest"` (Ollama's default tag).
fn model_name_matches(name: &str, target: &str) -> bool {
    name == target || name == format!("{target}:latest")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn default_latest_tag_is_equivalent() {
        assert!(model_name_matches("cue-s1-mini:latest", "cue-s1-mini"));
        assert!(model_name_matches("cue-s1-mini", "cue-s1-mini"));
        assert!(!model_name_matches("cue-s1-mini:other", "cue-s1-mini"));
        assert!(!model_name_matches("qwen3:8b", "cue-s1-mini"));
    }

    #[tokio::test]
    async fn native_chat_disables_thinking_and_returns_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(body_partial_json(serde_json::json!({
                "model": "cue-s1-mini",
                "stream": false,
                "think": false,
                "options": {"temperature": 0.0}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"message":{"role":"assistant","content":"Clean text."},"prompt_eval_count":10,"eval_count":3}"#,
            ))
            .mount(&server)
            .await;

        let response = OllamaChatClient::new(server.uri())
            .chat("cue-s1-mini", &[ChatMessage::user("raw text")], Some(0.0))
            .await
            .unwrap();

        assert_eq!(response.content, "Clean text.");
        assert_eq!(response.prompt_tokens, Some(10));
        assert_eq!(response.completion_tokens, Some(3));
    }

    #[tokio::test]
    async fn native_chat_http_error_reports_status_without_response_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(500).set_body_string("private-transcript-sentinel"))
            .mount(&server)
            .await;

        let err = OllamaChatClient::new(server.uri())
            .chat("cue-s1-mini", &[ChatMessage::user("raw text")], Some(0.0))
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("500"), "{err}");
        assert!(!err.contains("private-transcript-sentinel"), "{err}");
    }

    #[tokio::test]
    async fn native_chat_rejects_malformed_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .mount(&server)
            .await;

        let err = OllamaChatClient::new(server.uri())
            .chat("cue-s1-mini", &[ChatMessage::user("raw text")], Some(0.0))
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("unreadable chat output"), "{err}");
    }

    #[tokio::test]
    async fn lists_models_from_tags_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"models":[{"name":"s1-mini:latest"},{"name":"qwen3:8b"}]}"#,
                ),
            )
            .mount(&server)
            .await;

        let admin = OllamaAdmin::new(server.uri());
        let models = admin.list_models().await.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "s1-mini:latest");
        assert!(admin.has_model("qwen3:8b").await.unwrap());
        assert!(!admin.has_model("missing").await.unwrap());
    }

    #[tokio::test]
    async fn pull_posts_non_streaming_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/pull"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"status\":\"success\"}"))
            .mount(&server)
            .await;

        let admin = OllamaAdmin::new(server.uri());
        admin
            .pull("hf.co/superwhisper/s1-mini-GGUF:Q4_K_M")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_sends_modelfile_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/create"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"status\":\"success\"}"))
            .mount(&server)
            .await;

        let admin = OllamaAdmin::new(server.uri());
        admin
            .create(
                "cue-s1-mini",
                "FROM hf.co/superwhisper/s1-mini-GGUF:Q4_K_M\n",
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn unreachable_server_is_actionable() {
        let admin = OllamaAdmin::new("http://127.0.0.1:9"); // discard port
        let err = admin.list_models().await.unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains("could not reach Ollama"), "{rendered}");
        assert!(rendered.contains("is Ollama running?"), "{rendered}");
    }

    #[tokio::test]
    async fn tags_http_error_reports_status_without_response_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(500).set_body_string("private-transcript-sentinel"))
            .mount(&server)
            .await;

        let err = OllamaAdmin::new(server.uri())
            .list_models()
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("500"), "{err}");
        assert!(!err.contains("private-transcript-sentinel"), "{err}");
    }

    #[tokio::test]
    async fn malformed_tags_are_distinct_from_an_unreachable_server() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let err = OllamaAdmin::new(server.uri())
            .list_models()
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("unreadable output"), "{err}");
        assert!(!err.contains("could not reach Ollama"), "{err}");
    }
}
