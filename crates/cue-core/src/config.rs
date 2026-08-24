//! Configuration model and layered resolution.
//!
//! Precedence, highest first:
//!
//! ```text
//! CLI arguments  ->  environment  ->  user configuration  ->  defaults
//! ```
//!
//! Each layer is a [`PartialConfig`] where every field is optional; only
//! explicitly-set values take part in merging. The resolved [`Config`] is
//! total: every field has a value after [`resolve`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CueError;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Config {
    pub transcription: TranscriptionConfig,
    pub normalization: NormalizationConfig,
    pub subtitles: SubtitlesConfig,
    /// `None` means no LLM gateway is configured; local processing must
    /// succeed without one.
    pub llm: Option<LlmConfig>,
    pub analysis: AnalysisConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionConfig {
    #[serde(default = "default_transcription_provider")]
    pub provider: String,
    #[serde(default = "default_transcription_model")]
    pub model: String,
}

fn default_transcription_provider() -> String {
    "faster-whisper".into()
}

fn default_transcription_model() -> String {
    "large-v3-turbo".into()
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            provider: default_transcription_provider(),
            model: default_transcription_model(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizationConfig {
    #[serde(default = "default_normalization_provider")]
    pub provider: String,
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,
}

fn default_normalization_provider() -> String {
    "s1".into()
}

fn default_ollama_url() -> String {
    "http://localhost:11434".into()
}

impl Default for NormalizationConfig {
    fn default() -> Self {
        Self {
            provider: default_normalization_provider(),
            ollama_url: default_ollama_url(),
        }
    }
}

/// Policy for turning timed words into subtitle cues.
///
/// Defaults follow common subtitle practice; they are deliberately not
/// language-aware.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubtitlesConfig {
    #[serde(default = "default_subtitle_formats")]
    pub formats: Vec<SubtitleFormat>,
    #[serde(default = "default_max_lines")]
    pub max_lines: usize,
    #[serde(default = "default_max_chars_per_line")]
    pub max_chars_per_line: usize,
    #[serde(default = "default_max_duration_ms")]
    pub max_duration_ms: u64,
    pub max_chars_per_second: Option<f32>,
}

fn default_subtitle_formats() -> Vec<SubtitleFormat> {
    vec![SubtitleFormat::Srt, SubtitleFormat::Vtt]
}

fn default_max_lines() -> usize {
    2
}

fn default_max_chars_per_line() -> usize {
    42
}

fn default_max_duration_ms() -> u64 {
    6_000
}

impl Default for SubtitlesConfig {
    fn default() -> Self {
        Self {
            formats: default_subtitle_formats(),
            max_lines: default_max_lines(),
            max_chars_per_line: default_max_chars_per_line(),
            max_duration_ms: default_max_duration_ms(),
            max_chars_per_second: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubtitleFormat {
    Srt,
    Vtt,
}

impl SubtitleFormat {
    pub fn extension(self) -> &'static str {
        match self {
            SubtitleFormat::Srt => "srt",
            SubtitleFormat::Vtt => "vtt",
        }
    }

    pub fn parse(value: &str) -> Result<Self, CueError> {
        match value.to_ascii_lowercase().as_str() {
            "srt" => Ok(SubtitleFormat::Srt),
            "vtt" => Ok(SubtitleFormat::Vtt),
            other => Err(CueError::general(format!(
                "unknown subtitle format \"{other}\""
            ))
            .remedy("supported formats are \"srt\" and \"vtt\"")),
        }
    }
}

/// Connection details for an OpenAI-compatible LLM gateway.
///
/// Works unchanged against Ollama (`http://localhost:11434/v1`) and remote
/// gateways such as OpenRouter (`https://openrouter.ai/api/v1`). The API key
/// is never stored in configuration files; it is read from the environment
/// variable named by `api_key_env`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmConfig {
    pub base_url: String,
    pub model: String,
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
}

fn default_api_key_env() -> String {
    "CUE_LLM_API_KEY".into()
}

impl LlmConfig {
    /// Reads the API key from the environment variable named by
    /// `api_key_env`, if that variable is set.
    pub fn api_key(&self) -> Option<String> {
        std::env::var(&self.api_key_env).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalysisConfig {
    #[serde(default = "default_true")]
    pub summary: bool,
    #[serde(default = "default_true")]
    pub description: bool,
    #[serde(default = "default_true")]
    pub chapters: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            summary: true,
            description: true,
            chapters: true,
        }
    }
}


/// A configuration layer where every field is optional.
///
/// Fields left as `None` fall through to lower-precedence layers.
/// The doubly-optional `llm` field distinguishes "not mentioned in this
/// layer" (`None`) from "explicitly disabled here" (`Some(None)`), which the
/// CLI uses for a future `--no-llm` flag.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartialConfig {
    pub transcription: PartialTranscription,
    pub normalization: PartialNormalization,
    pub subtitles: PartialSubtitles,
    pub llm: Option<Option<LlmConfig>>,
    pub analysis: PartialAnalysis,
}

macro_rules! partial_section {
    ($partial:ident, $total:ident, $($field:ident : $ty:ty),* $(,)?) => {
        #[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
        pub struct $partial {
            $(pub $field: Option<$ty>,)*
        }

        impl $partial {
            /// Overlay this section onto a fully-populated section.
            pub fn apply_to(self, target: &mut $total) {
                $(if let Some(v) = self.$field { target.$field = v; })*
            }

            /// Take the set fields of a total section into a partial layer.
            pub fn from_total(total: &$total) -> Self {
                Self { $($field: Some(total.$field.clone()),)* }
            }
        }
    };
}

partial_section!(
    PartialTranscription,
    TranscriptionConfig,
    provider: String,
    model: String,
);

partial_section!(
    PartialNormalization,
    NormalizationConfig,
    provider: String,
    ollama_url: String,
);

partial_section!(
    PartialSubtitles,
    SubtitlesConfig,
    formats: Vec<SubtitleFormat>,
    max_lines: usize,
    max_chars_per_line: usize,
    max_duration_ms: u64,
    max_chars_per_second: Option<f32>,
);

partial_section!(
    PartialAnalysis,
    AnalysisConfig,
    summary: bool,
    description: bool,
    chapters: bool,
);

/// Merge layers into a total config. Earlier layers win.
pub fn resolve(layers: &[&PartialConfig]) -> Config {
    let mut config = Config::default();
    for layer in layers.iter().rev() {
        layer.transcription.clone().apply_to(&mut config.transcription);
        layer.normalization.clone().apply_to(&mut config.normalization);
        layer.subtitles.clone().apply_to(&mut config.subtitles);
        if let Some(llm) = &layer.llm {
            config.llm = llm.clone();
        }
        layer.analysis.clone().apply_to(&mut config.analysis);
    }
    config
}

/// Parse user configuration from TOML text.
pub fn parse_toml(text: &str) -> Result<PartialConfig, CueError> {
    let raw: RawToml = toml::from_str(text)
        .map_err(|e| CueError::general("configuration file is not valid TOML").because(e.to_string()).remedy("check the syntax of your cue.toml"))?;
    raw.finish()
}

/// Location of the user configuration file:
/// `$CUE_CONFIG_DIR/cue.toml`, else `$XDG_CONFIG_HOME/cue/cue.toml`,
/// else `~/.config/cue/cue.toml`.
pub fn user_config_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CUE_CONFIG_DIR") {
        return Some(Path::new(&dir).join("cue.toml"));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".config")))?;
    Some(base.join("cue").join("cue.toml"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Read and parse the user config file. Missing file yields the empty layer.
pub fn load_user_config() -> Result<PartialConfig, CueError> {
    match user_config_path() {
        Some(path) if path.exists() => {
            let text = std::fs::read_to_string(&path).map_err(|e| {
                CueError::general(format!(
                    "could not read configuration file {}",
                    path.display()
                ))
                .because(e.to_string())
                .remedy("check the file's permissions")
            })?;
            parse_toml(&text)
        }
        _ => Ok(PartialConfig::default()),
    }
}

// Serde view over the TOML file shape. Mirrors plan section 19 exactly so
// users' files look like the documented example.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawToml {
    #[serde(default)]
    transcription: PartialTranscription,
    #[serde(default)]
    normalization: PartialNormalization,
    #[serde(default)]
    subtitles: PartialSubtitlesRaw,
    #[serde(default)]
    llm: Option<LlmConfig>,
    #[serde(default)]
    analysis: PartialAnalysis,
}

// TOML lists subtitle formats as strings; validate them explicitly so an
// unknown format produces a proper error rather than a serde failure.
#[derive(Debug, Default, Deserialize)]
struct PartialSubtitlesRaw {
    #[serde(default)]
    formats: Option<Vec<String>>,
    #[serde(flatten)]
    rest: PartialSubtitles,
}

impl RawToml {
    fn finish(self) -> Result<PartialConfig, CueError> {
        let formats = match self.subtitles.formats {
            Some(names) => Some(
                names
                    .iter()
                    .map(|f| SubtitleFormat::parse(f))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            None => None,
        };
        let mut subtitles = self.subtitles.rest;
        subtitles.formats = formats;
        Ok(PartialConfig {
            transcription: self.transcription,
            normalization: self.normalization,
            subtitles,
            llm: self.llm.map(Some),
            analysis: self.analysis,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_plan() {
        let config = Config::default();
        assert_eq!(config.transcription.provider, "faster-whisper");
        assert_eq!(config.transcription.model, "large-v3-turbo");
        assert_eq!(config.normalization.provider, "s1");
        assert_eq!(config.normalization.ollama_url, "http://localhost:11434");
        assert_eq!(
            config.subtitles.formats,
            vec![SubtitleFormat::Srt, SubtitleFormat::Vtt]
        );
        assert_eq!(config.subtitles.max_lines, 2);
        assert_eq!(config.subtitles.max_chars_per_line, 42);
        assert_eq!(config.subtitles.max_duration_ms, 6_000);
        assert_eq!(config.llm, None);
    }

    #[test]
    fn resolve_empty_layers_yields_defaults() {
        let config = resolve(&[&PartialConfig::default(), &PartialConfig::default()]);
        assert_eq!(config, Config::default());
    }

    #[test]
    fn earlier_layers_win() {
        let file_layer = PartialConfig {
            transcription: PartialTranscription {
                model: Some("small".into()),
                ..Default::default()
            },
            llm: Some(Some(LlmConfig {
                base_url: "http://from-file".into(),
                model: "file-model".into(),
                api_key_env: "FILE_KEY".into(),
            })),
            ..Default::default()
        };
        let cli_layer = PartialConfig {
            transcription: PartialTranscription {
                model: Some("tiny".into()),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = resolve(&[&cli_layer, &file_layer]);
        // CLI wins over file...
        assert_eq!(config.transcription.model, "tiny");
        // ...but untouched CLI fields fall through to the file layer.
        assert_eq!(config.transcription.provider, "faster-whisper");
        assert_eq!(config.llm.as_ref().unwrap().base_url, "http://from-file");
    }

    #[test]
    fn cli_can_explicitly_disable_llm() {
        let no_llm = PartialConfig {
            llm: Some(None),
            ..Default::default()
        };
        let with_llm = PartialConfig {
            llm: Some(Some(LlmConfig {
                base_url: "x".into(),
                model: "y".into(),
                api_key_env: "Z".into(),
            })),
            ..Default::default()
        };
        let config = resolve(&[&no_llm, &with_llm]);
        assert_eq!(config.llm, None);
    }

    #[test]
    fn parses_plan_example_toml() {
        let text = r#"
[transcription]
provider = "faster-whisper"
model = "large-v3-turbo"

[normalization]
provider = "s1"
ollama_url = "http://localhost:11434"

[subtitles]
formats = ["srt", "vtt"]
max_lines = 2
max_chars_per_line = 42

[llm]
base_url = "https://gateway.example.com/v1"
model = "model-name"
api_key_env = "CUE_LLM_API_KEY"

[analysis]
summary = true
description = true
chapters = true
"#;
        let partial = parse_toml(text).unwrap();
        let config = resolve(&[&partial]);

        assert_eq!(config.transcription.model, "large-v3-turbo");
        assert_eq!(config.normalization.ollama_url, "http://localhost:11434");
        assert_eq!(config.subtitles.max_chars_per_line, 42);
        assert_eq!(
            config.subtitles.formats,
            vec![SubtitleFormat::Srt, SubtitleFormat::Vtt]
        );
        let llm = config.llm.unwrap();
        assert_eq!(llm.base_url, "https://gateway.example.com/v1");
        assert_eq!(llm.model, "model-name");
        assert_eq!(llm.api_key_env, "CUE_LLM_API_KEY");
        assert!(config.analysis.summary && config.analysis.chapters);
    }

    #[test]
    fn invalid_toml_reports_remedy() {
        let err = parse_toml("[transcription\nmodel=").unwrap_err();
        assert!(err.to_string().contains("not valid TOML"), "{err}");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err = parse_toml("[transcrption]\nmodel = \"x\"").unwrap_err();
        assert!(err.to_string().contains("not valid TOML"), "{err}");
    }

    #[test]
    fn invalid_subtitle_format_is_an_error_not_a_panic() {
        let err = parse_toml("[subtitles]\nformats = [\"srt\", \"ass\"]").unwrap_err();
        assert!(
            err.to_string().contains("unknown subtitle format \"ass\""),
            "{err}"
        );
    }

    #[test]
    fn subtitle_formats_round_trip() {
        assert_eq!(SubtitleFormat::Srt.extension(), "srt");
        assert_eq!(SubtitleFormat::parse("VTT").unwrap(), SubtitleFormat::Vtt);
    }
}
