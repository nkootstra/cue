//! OpenAI-compatible chat completions.
//!
//! One client covers Ollama (`http://localhost:11434/v1`) and remote
//! gateways such as OpenRouter (`https://openrouter.ai/api/v1`): they speak
//! the same wire protocol, differing only in base URL and authentication.

use serde::{Deserialize, Serialize};

use cue_core::{CueError, Result};

/// A single chat message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }
}

/// Non-streaming chat completion request.
#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

/// Response carrying the assistant text plus token usage when reported.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatResponse {
    pub content: String,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

/// Client bound to one base URL and optional API key.
#[derive(Debug, Clone)]
pub struct ChatClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl ChatClient {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: normalize_base(base_url.into()),
            api_key,
        }
    }

    /// Send a non-streaming chat completion.
    ///
    /// Errors name the gateway and keep response bodies out of the default
    /// rendering so transcript contents never leak into logs.
    pub async fn chat(
        &self,
        model: &str,
        messages: &[ChatMessage],
        temperature: Option<f32>,
    ) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut request = self.http.post(&url).json(&ChatRequest {
            model,
            messages,
            temperature,
        });
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        // Local models can be slow to load on first use.
        let response = request
            .timeout(std::time::Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| gateway_error("could not reach the LLM gateway", e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(CueError::new(
                cue_core::PipelineStage::Analyze,
                format!("the LLM gateway rejected a request ({status})"),
            )
            .remedy("verify the configured gateway URL, model name, and API key"));
        }

        let parsed: WireResponse = response
            .json()
            .await
            .map_err(|e| gateway_error("gateway returned unreadable JSON", e.to_string()))?;

        let content = parsed
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        Ok(ChatResponse {
            content,
            prompt_tokens: parsed.usage.as_ref().map(|u| u.prompt_tokens),
            completion_tokens: parsed.usage.as_ref().map(|u| u.completion_tokens),
        })
    }
}

fn normalize_base(mut url: String) -> String {
    while url.ends_with('/') {
        url.pop();
    }
    url
}

fn gateway_error(summary: &str, reason: String) -> CueError {
    CueError::new(cue_core::PipelineStage::Analyze, summary).because(reason)
}

// Wire subset of the OpenAI chat completion response.
#[derive(Debug, Deserialize)]
struct WireResponse {
    choices: Vec<WireChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Debug, Deserialize)]
struct WireChoice {
    message: WireMessage,
}

#[derive(Debug, Deserialize)]
struct WireMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct WireUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const WIRE_OK: &str = r#"{
        "id": "x", "object": "chat.completion",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "clean text"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 4}
    }"#;

    #[tokio::test]
    async fn posts_messages_and_parses_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_partial_json(serde_json::json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "raw input"}]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_string(WIRE_OK))
            .mount(&server)
            .await;

        let client = ChatClient::new(format!("{}/v1", server.uri()), None);
        let response = client
            .chat("test-model", &[ChatMessage::user("raw input")], Some(0.0))
            .await
            .unwrap();

        assert_eq!(response.content, "clean text");
        assert_eq!(response.prompt_tokens, Some(10));
        assert_eq!(response.completion_tokens, Some(4));
    }

    #[tokio::test]
    async fn sends_bearer_auth_when_key_present() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_string(WIRE_OK))
            .mount(&server)
            .await;

        let client = ChatClient::new(server.uri(), Some("sk-test".into()));
        let result = client.chat("m", &[ChatMessage::user("x")], None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn gateway_error_does_not_echo_response_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_string(r#"{"error":"bad key for secret-input"}"#),
            )
            .mount(&server)
            .await;

        let client = ChatClient::new(server.uri(), Some("wrong".into()));
        let err = client
            .chat("m", &[ChatMessage::user("secret content")], None)
            .await
            .unwrap_err();
        let rendered = err.to_string();
        // The failing response body must not leak into the rendered error.
        assert!(!rendered.contains("secret-input"), "{rendered}");
        assert!(rendered.contains("401"), "{rendered}");
        assert!(!rendered.contains("response body"), "{rendered}");
    }

    #[test]
    fn trailing_slashes_are_normalized() {
        assert_eq!(
            ChatClient::new("http://localhost:11434/v1/", None).base_url,
            "http://localhost:11434/v1"
        );
    }
}
