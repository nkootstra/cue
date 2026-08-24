//! Structured analysis of normalized transcripts through an LLM gateway.
//!
//! Long transcripts use a map/reduce flow: each chunk yields a partial
//! analysis, and a synthesis pass merges partials into one grounded
//! [`Analysis`]. Short transcripts skip the map stage entirely.

use async_trait::async_trait;
use cue_core::{Analysis, NormalizedTranscript};
use tracing::instrument;

use crate::json::parse_lenient_json;

/// Bump when prompt wording changes in a way that could alter output; this
/// participates in analysis cache keys.
pub const PROMPT_VERSION: u32 = 1;

/// Chars per analysis chunk. Sized to keep prompts well within small local
/// model contexts while limiting request count.
const CHUNK_CHARS: usize = 4_000;

/// A provider that produces a structured [`Analysis`] from cleaned text.
#[async_trait]
pub trait Analyzer: Send + Sync {
    fn name(&self) -> &str;
    async fn analyze(&self, input: &AnalysisInput) -> cue_core::Result<Analysis>;
}

/// What an analyzer sees: cleaned text spans with their time ranges.
#[derive(Debug, Clone)]
pub struct AnalysisInput {
    pub language: String,
    pub spans: Vec<SpanText>,
}

#[derive(Debug, Clone)]
pub struct SpanText {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

impl AnalysisInput {
    /// From a normalized transcript.
    pub fn from_normalized(normalized: &NormalizedTranscript) -> Self {
        Self {
            language: "en".to_string(),
            spans: normalized
                .chunks
                .iter()
                .map(|c| SpanText {
                    start_ms: c.start_ms,
                    end_ms: c.end_ms,
                    text: c.text.clone(),
                })
                .collect(),
        }
    }

    /// Group spans into roughly `CHUNK_CHARS`-sized analysis chunks,
    /// preserving order and timestamps.
    fn chunks(&self) -> Vec<AnalysisInput> {
        let mut chunks = Vec::new();
        let mut current: Vec<SpanText> = Vec::new();
        let mut chars = 0usize;

        for span in &self.spans {
            if chars > 0 && chars + span.text.chars().count() > CHUNK_CHARS {
                chunks.push(AnalysisInput {
                    language: self.language.clone(),
                    spans: std::mem::take(&mut current),
                });
                chars = 0;
            }
            chars += span.text.chars().count();
            current.push(span.clone());
        }
        if !current.is_empty() {
            chunks.push(AnalysisInput {
                language: self.language.clone(),
                spans: current,
            });
        }
        chunks
    }
}

/// Analyzes through any OpenAI-compatible gateway (Ollama `/v1`, OpenRouter).
pub struct GatewayAnalyzer {
    client: cue_llm::ChatClient,
    model: String,
}

impl GatewayAnalyzer {
    pub fn new(client: cue_llm::ChatClient, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }

    async fn complete_json(&self, system: &str, user: String) -> cue_core::Result<String> {
        let response = self
            .client
            .chat(
                &self.model,
                &[
                    cue_llm::ChatMessage::system(system),
                    cue_llm::ChatMessage::user(user),
                ],
                Some(0.2),
            )
            .await?;
        Ok(response.content)
    }
}

const SCHEMA_INSTRUCTION: &str = r#"Respond with JSON only (no markdown fences) matching:
{"language": string, "title": string, "summary": string,
 "topics": [{"start_ms": int, "end_ms": int, "title": string, "summary": string, "key_points": [string]}],
 "key_points": [string], "keywords": [string]}"#;

const MAP_SYSTEM: &str = "You analyze transcripts of videos. Given a transcript excerpt with its time range, produce a partial analysis grounded in the text. Use the timestamps you see; do not invent times.";
const REDUCE_SYSTEM: &str = "You synthesize partial video analyses into one coherent analysis. Merge topics that cover the same subject, keeping their earliest start_ms and latest end_ms from the partials. Ground every timestamp in the partials; never invent times.";

#[async_trait]
impl Analyzer for GatewayAnalyzer {
    fn name(&self) -> &str {
        "gateway"
    }

    #[instrument(skip(self, input))]
    async fn analyze(&self, input: &AnalysisInput) -> cue_core::Result<Analysis> {
        let chunks = input.chunks();

        let raw = match chunks.len() {
            0 => return Ok(empty_analysis(&input.language)),
            // Short content: single direct request, no map stage.
            1 => self.complete_json(SCHEMA_INSTRUCTION, direct_prompt(input)).await?,
            _ => {
                let mut combined = String::from(
                    "Partial analyses of consecutive transcript excerpts follow.\n\n",
                );
                for (index, chunk) in chunks.iter().enumerate() {
                    let partial_raw = self
                        .complete_json(
                            MAP_SYSTEM,
                            format!(
                                "{SCHEMA_INSTRUCTION}\n\nTranscript excerpt {index}:\n{}",
                                render_spans(chunk)
                            ),
                        )
                        .await?;
                    combined.push_str(&format!(
                        "--- Partial {index} ---\n{partial_raw}\n\n"
                    ));
                }
                combined.push_str(
                    "Synthesize these partials into a single analysis for the whole video.",
                );
                self.complete_json(REDUCE_SYSTEM, combined).await?
            }
        };

        parse_analysis(&raw, &input.language)
    }
}

fn render_spans(input: &AnalysisInput) -> String {
    let mut out = String::new();
    for span in &input.spans {
        out.push_str(&format!(
            "[{:08.3}s - {:08.3}s] {}\n",
            span.start_ms as f64 / 1000.0,
            span.end_ms as f64 / 1000.0,
            span.text
        ));
    }
    out
}

fn direct_prompt(input: &AnalysisInput) -> String {
    format!(
        "{SCHEMA_INSTRUCTION}\n\nVideo transcript:\n{}",
        render_spans(input)
    )
}

fn empty_analysis(language: &str) -> Analysis {
    Analysis {
        schema_version: cue_core::ANALYSIS_SCHEMA_VERSION,
        language: language.to_string(),
        title: "Untitled".into(),
        summary: String::new(),
        topics: vec![],
        key_points: vec![],
        keywords: vec![],
    }
}

/// Parse model output into an Analysis, tolerating markdown fences and
/// falling back to defaults per-field where possible.
fn parse_analysis(raw: &str, language: &str) -> cue_core::Result<Analysis> {
    let json = parse_lenient_json(raw)?;
    let mut analysis: Analysis =
        serde_json::from_value(json).map_err(|e| {
            cue_core::CueError::new(
                cue_core::PipelineStage::Analyze,
                "analysis did not match the expected schema",
            )
            .because(e.to_string())
            .remedy("retry, or try a different gateway model")
        })?;
    if analysis.schema_version != cue_core::ANALYSIS_SCHEMA_VERSION {
        analysis.schema_version = cue_core::ANALYSIS_SCHEMA_VERSION;
    }
    if analysis.language.is_empty() {
        analysis.language = language.to_string();
    }
    Ok(analysis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn analysis_json(title: &str) -> String {
        format!(
            r#"{{"language":"en","title":"{title}","summary":"A summary.",
                "topics":[{{"start_ms":0,"end_ms":1000,"title":"Part one","summary":"","key_points":[]}}],
                "key_points":["point"],"keywords":["kw"]}}"#
        )
    }

    fn input(spans: &[(&str, u64, u64)]) -> AnalysisInput {
        AnalysisInput {
            language: "en".into(),
            spans: spans
                .iter()
                .map(|(text, start, end)| SpanText {
                    text: text.to_string(),
                    start_ms: *start,
                    end_ms: *end,
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn short_input_skips_map_stage() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("Video transcript"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(format!(
                    r#"{{"choices":[{{"message":{{"role":"assistant","content":{}}}}}]}}"#,
                    serde_json::to_string(&analysis_json("Direct")).unwrap()
                )),
            )
            .mount(&server)
            .await;

        let analyzer =
            GatewayAnalyzer::new(cue_llm::ChatClient::new(server.uri(), None), "test-model");
        let result = analyzer.analyze(&input(&[("hello world.", 0, 1_000)])).await.unwrap();
        assert_eq!(result.title, "Direct");
        assert_eq!(result.chapters().len(), 1);
    }

    #[tokio::test]
    async fn long_input_maps_then_reduces_with_grounding_instruction() {
        let server = MockServer::start().await;

        // Map stage requests mention excerpts; respond with partial JSON.
        Mock::given(method("POST"))
            .and(body_string_contains("Transcript excerpt"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"choices":[{"message":{"role":"assistant",
                   "content":"{\"language\":\"\",\"title\":\"partial\",\"summary\":\"s\",\"topics\":[],\"key_points\":[],\"keywords\":[]}"}}]}"#,
            ))
            .mount(&server)
            .await;

        // Reduce request asks for synthesis; return the merged analysis.
        Mock::given(method("POST"))
            .and(body_string_contains("Synthesize these partials"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                format!(
                    r#"{{"choices":[{{"message":{{"content":{}}}}}]}}"#,
                    serde_json::to_string(&analysis_json("Merged")).unwrap()
                )
            ))
            .mount(&server)
            .await;

        let analyzer =
            GatewayAnalyzer::new(cue_llm::ChatClient::new(server.uri(), None), "test-model");

        // Two spans exceed CHUNK_CHARS only if large; shrink by using the
        // chunker indirectly — feed enough text to force two chunks.
        let big = "x".repeat(CHUNK_CHARS + 10);
        let result = analyzer
            .analyze(&input(&[(&big, 0, 5_000), ("tail.", 5_000, 6_000)]))
            .await
            .unwrap();

        assert_eq!(result.title, "Merged");
        assert_eq!(result.schema_version, cue_core::ANALYSIS_SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn fenced_responses_parse() {
        let server = MockServer::start().await;
        let fenced = format!("```json\n{}\n```", analysis_json("Fenced"));
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                r#"{{"choices":[{{"message":{{"content":{}}}}}]}}"#,
                serde_json::to_string(&fenced).unwrap()
            )))
            .mount(&server)
            .await;

        let analyzer =
            GatewayAnalyzer::new(cue_llm::ChatClient::new(server.uri(), None), "m");
        let result = analyzer.analyze(&input(&[("text.", 0, 100)])).await.unwrap();
        assert_eq!(result.title, "Fenced");
    }

    #[test]
    fn chunking_groups_spans_in_order() {
        let input = input(&[("a", 0, 1), ("b", 1, 2), ("c", 2, 3)]);
        let chunks = input.chunks();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].spans.len(), 3);
        assert!(chunks[0].spans[0].end_ms <= chunks[0].spans[1].start_ms);
    }

    #[tokio::test]
    async fn empty_input_yields_empty_analysis_without_network() {
        let analyzer =
            GatewayAnalyzer::new(cue_llm::ChatClient::new("http://127.0.0.1:9", None), "m");
        let result = analyzer.analyze(&AnalysisInput { language: "en".into(), spans: vec![] }).await;
        // No request is made; an empty analysis comes back.
        assert!(result.is_ok());
    }
}
