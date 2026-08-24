//! Ollama administration: list, pull, and create models.
//!
//! These operations are Ollama-specific (no OpenRouter equivalent) and back
//! `cue models list/check/install`.

use serde::{Deserialize, Serialize};

use cue_core::{CueError, Result};

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

    /// Names of models already present locally.
    pub async fn list_models(&self) -> Result<Vec<OllamaModel>> {
        let response: TagsResponse = self
            .http
            .get(format!("{}/api/tags", self.base_url))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| ollama_unreachable(e.to_string()))?
            .json()
            .await
            .map_err(|e| ollama_error("Ollama returned unreadable output", e.to_string()))?;

        Ok(response
            .models
            .into_iter()
            .map(|m| OllamaModel { name: m.name })
            .collect())
    }

    /// True when a model with this exact name exists.
    pub async fn has_model(&self, name: &str) -> Result<bool> {
        Ok(self.list_models().await?.iter().any(|m| m.name == name))
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
            let status = response.status();
            return Err(ollama_error(
                format!("pulling {model_ref} failed ({status})"),
                "see the Ollama server log for details".into(),
            ));
        }
        Ok(())
    }

    /// Create a named model from Modelfile text.
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
            let status = response.status();
            return Err(ollama_error(
                format!("creating model {name} failed ({status})"),
                "check the Modelfile syntax and the Ollama server log".into(),
            ));
        }
        Ok(())
    }
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
struct TagsModel {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn lists_models_from_tags_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"models":[{"name":"s1-mini:latest"},{"name":"qwen3:8b"}]}"#,
            ))
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
}
