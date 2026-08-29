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
use url::{Host, Url};

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
    /// S1 control-line styling; see the superwhisper/s1-mini model card.
    #[serde(default = "default_s1_styling")]
    pub styling: String,
    /// S1 control-line structure.
    #[serde(default = "default_s1_structure")]
    pub structure: String,
    /// S1 control-line context.
    #[serde(default = "default_s1_context")]
    pub context: String,
}

fn default_normalization_provider() -> String {
    "s1".into()
}

fn default_ollama_url() -> String {
    "http://localhost:11434".into()
}

fn default_s1_styling() -> String {
    "semi-formal".into()
}

fn default_s1_structure() -> String {
    "prose".into()
}

fn default_s1_context() -> String {
    "general".into()
}

impl Default for NormalizationConfig {
    fn default() -> Self {
        Self {
            provider: default_normalization_provider(),
            ollama_url: default_ollama_url(),
            styling: default_s1_styling(),
            structure: default_s1_structure(),
            context: default_s1_context(),
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
}

fn default_subtitle_formats() -> Vec<SubtitleFormat> {
    vec![SubtitleFormat::Srt]
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
            other => Err(
                CueError::general(format!("unknown subtitle format \"{other}\""))
                    .remedy("supported formats are \"srt\" and \"vtt\""),
            ),
        }
    }
}

impl std::str::FromStr for SubtitleFormat {
    type Err = CueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl std::fmt::Display for SubtitleFormat {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.extension())
    }
}

/// Connection details for an OpenAI-compatible LLM gateway.
///
/// Works unchanged against Ollama (`http://localhost:11434/v1`) and remote
/// gateways such as OpenRouter (`https://openrouter.ai/api/v1`). When
/// `api_key_env` is nonempty, the API key is read from that environment
/// variable and never stored in configuration files. An empty or whitespace-
/// only value explicitly selects an unauthenticated gateway.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmConfig {
    pub base_url: String,
    pub model: String,
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
}

/// Whether a configured LLM gateway has the credentials it declares.
///
/// This status deliberately carries only the environment-variable name, never
/// the credential value itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmCredentialReadiness {
    /// The gateway explicitly declares that it does not require credentials.
    Unauthenticated,
    /// The named credential is available in the process environment.
    Available { api_key_env: String },
    /// The gateway requires a named credential that is not available.
    Missing { api_key_env: String },
}

fn default_api_key_env() -> String {
    "CUE_LLM_API_KEY".into()
}

impl LlmConfig {
    fn declared_api_key_env(&self) -> Option<&str> {
        let api_key_env = self.api_key_env.trim();
        (!api_key_env.is_empty()).then_some(api_key_env)
    }

    fn declared_api_key(&self) -> Option<String> {
        self.declared_api_key_env()
            .and_then(|api_key_env| std::env::var(api_key_env).ok())
            .filter(|api_key| !api_key.trim().is_empty())
    }

    /// Reports whether the gateway's declared credential is ready for use.
    pub fn credential_readiness(&self) -> LlmCredentialReadiness {
        let Some(api_key_env) = self.declared_api_key_env() else {
            return LlmCredentialReadiness::Unauthenticated;
        };
        if self.declared_api_key().is_some() {
            return LlmCredentialReadiness::Available {
                api_key_env: api_key_env.into(),
            };
        }
        LlmCredentialReadiness::Missing {
            api_key_env: api_key_env.into(),
        }
    }

    /// Reads the API key from the environment variable named by
    /// `api_key_env`, if that variable is set.
    pub fn api_key(&self) -> Option<String> {
        self.declared_api_key()
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
    styling: String,
    structure: String,
    context: String,
);

partial_section!(
    PartialSubtitles,
    SubtitlesConfig,
    formats: Vec<SubtitleFormat>,
    max_lines: usize,
    max_chars_per_line: usize,
    max_duration_ms: u64,
);

partial_section!(
    PartialAnalysis,
    AnalysisConfig,
    summary: bool,
    description: bool,
    chapters: bool,
);

/// Merge layers into a total config. Earlier layers win.
pub fn resolve(layers: &[&PartialConfig]) -> Result<Config, CueError> {
    let mut config = Config::default();
    for layer in layers.iter().rev() {
        layer
            .transcription
            .clone()
            .apply_to(&mut config.transcription);
        layer
            .normalization
            .clone()
            .apply_to(&mut config.normalization);
        layer.subtitles.clone().apply_to(&mut config.subtitles);
        if let Some(llm) = &layer.llm {
            config.llm = llm.clone();
        }
        layer.analysis.clone().apply_to(&mut config.analysis);
    }
    validate_normalization(&config.normalization)?;
    validate_subtitles(&config.subtitles)?;
    Ok(config)
}

fn validate_normalization(config: &NormalizationConfig) -> Result<(), CueError> {
    let url = Url::parse(&config.ollama_url).map_err(|error| {
        CueError::general("normalization.ollama_url must be a valid local URL")
            .because(error.to_string())
            .remedy("set normalization.ollama_url to a localhost or loopback address")
    })?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err(
            CueError::general("normalization.ollama_url must be an HTTP(S) URL")
                .because(format!("{} uses an unsupported scheme", config.ollama_url))
                .remedy("set normalization.ollama_url to a local HTTP(S) Ollama endpoint"),
        );
    }

    let is_local = match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => {
            address.is_loopback()
                || address
                    .to_ipv4_mapped()
                    .is_some_and(|address| address.is_loopback())
        }
        None => false,
    };

    if !is_local {
        return Err(
            CueError::general("normalization.ollama_url must point to the local machine")
                .because(format!(
                    "{} is not a localhost or loopback address",
                    config.ollama_url
                ))
                .remedy("run Ollama locally and use a URL such as http://localhost:11434"),
        );
    }

    Ok(())
}

fn validate_subtitles(config: &SubtitlesConfig) -> Result<(), CueError> {
    for (field, value) in [
        ("max_lines", config.max_lines as u64),
        ("max_chars_per_line", config.max_chars_per_line as u64),
        ("max_duration_ms", config.max_duration_ms),
    ] {
        if value == 0 {
            return Err(
                CueError::general(format!("subtitles.{field} must be greater than zero")).remedy(
                    format!("set subtitles.{field} to a positive value in cue.toml"),
                ),
            );
        }
    }
    Ok(())
}

/// Parse user configuration from TOML text.
pub fn parse_toml(text: &str) -> Result<PartialConfig, CueError> {
    let raw: RawToml = toml::from_str(text).map_err(|e| {
        CueError::general("configuration file is not valid TOML")
            .because(e.to_string())
            .remedy("check the syntax of your cue.toml")
    })?;
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
    /// One-release compatibility input. Reading speed was never enforced,
    /// so retaining it in resolved state implied behavior that did not exist.
    #[serde(default)]
    max_chars_per_second: Option<f32>,
    #[serde(flatten)]
    rest: PartialSubtitles,
}

impl RawToml {
    fn finish(self) -> Result<PartialConfig, CueError> {
        if self.subtitles.max_chars_per_second.is_some() {
            tracing::warn!(
                "subtitles.max_chars_per_second is deprecated and ignored; remove it from cue.toml"
            );
        }
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
    fn llm_credentials_report_a_named_missing_variable() {
        let config = LlmConfig {
            base_url: "https://gateway.example.com/v1".into(),
            model: "test-model".into(),
            api_key_env: "  CUE_TEST_DEFINITELY_UNSET_CREDENTIAL_91A7  ".into(),
        };

        assert_eq!(
            config.credential_readiness(),
            LlmCredentialReadiness::Missing {
                api_key_env: "CUE_TEST_DEFINITELY_UNSET_CREDENTIAL_91A7".into(),
            }
        );
    }

    #[test]
    fn llm_credentials_use_a_trimmed_variable_without_exposing_its_value() {
        let config = LlmConfig {
            base_url: "https://gateway.example.com/v1".into(),
            model: "test-model".into(),
            api_key_env: "  PATH  ".into(),
        };

        let readiness = config.credential_readiness();
        assert_eq!(config.api_key(), std::env::var("PATH").ok());
        assert_eq!(
            readiness,
            LlmCredentialReadiness::Available {
                api_key_env: "PATH".into(),
            }
        );
        assert!(!format!("{readiness:?}").contains(&std::env::var("PATH").unwrap()));
    }

    #[test]
    fn llm_credentials_allow_explicitly_unauthenticated_gateways() {
        for api_key_env in ["", "   \t"] {
            let config = LlmConfig {
                base_url: "http://localhost:8765/v1".into(),
                model: "test-model".into(),
                api_key_env: api_key_env.into(),
            };

            assert_eq!(
                config.credential_readiness(),
                LlmCredentialReadiness::Unauthenticated
            );
            assert_eq!(config.api_key(), None);
        }
    }

    #[test]
    fn omitted_llm_key_declaration_keeps_the_default_variable() {
        let partial = parse_toml(
            r#"
[llm]
base_url = "https://gateway.example.com/v1"
model = "test-model"
"#,
        )
        .unwrap();

        let config = resolve(&[&partial]).unwrap();
        assert_eq!(config.llm.unwrap().api_key_env, "CUE_LLM_API_KEY");
    }

    #[test]
    fn defaults_match_the_plan() {
        let config = Config::default();
        assert_eq!(config.transcription.provider, "faster-whisper");
        assert_eq!(config.transcription.model, "large-v3-turbo");
        assert_eq!(config.normalization.provider, "s1");
        assert_eq!(config.normalization.ollama_url, "http://localhost:11434");
        assert_eq!(config.normalization.styling, "semi-formal");
        assert_eq!(config.normalization.structure, "prose");
        assert_eq!(config.normalization.context, "general");
        assert_eq!(config.subtitles.formats, vec![SubtitleFormat::Srt]);
        assert_eq!(config.subtitles.max_lines, 2);
        assert_eq!(config.subtitles.max_chars_per_line, 42);
        assert_eq!(config.subtitles.max_duration_ms, 6_000);
        assert_eq!(config.llm, None);
    }

    #[test]
    fn resolve_empty_layers_yields_defaults() {
        let config = resolve(&[&PartialConfig::default(), &PartialConfig::default()]).unwrap();
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

        let config = resolve(&[&cli_layer, &file_layer]).unwrap();
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
        let config = resolve(&[&no_llm, &with_llm]).unwrap();
        assert_eq!(config.llm, None);
    }

    #[test]
    fn parses_s1_control_line_knobs() {
        let partial = parse_toml(
            "[normalization]\nstyling = \"formal\"\nstructure = \"bullets\"\ncontext = \"technical\"\n",
        )
        .unwrap();
        let config = resolve(&[&partial]).unwrap();
        assert_eq!(config.normalization.styling, "formal");
        assert_eq!(config.normalization.structure, "bullets");
        assert_eq!(config.normalization.context, "technical");
    }

    #[test]
    fn normalization_accepts_localhost_and_loopback_urls() {
        for ollama_url in [
            "http://localhost:11434",
            "http://LOCALHOST:11434/api",
            "http://127.0.0.1:11434",
            "http://127.42.0.9:11434",
            "http://[::1]:11434",
            "http://[::ffff:127.0.0.1]:11434",
        ] {
            let layer = PartialConfig {
                normalization: PartialNormalization {
                    ollama_url: Some(ollama_url.into()),
                    ..Default::default()
                },
                ..Default::default()
            };

            resolve(&[&layer])
                .unwrap_or_else(|error| panic!("expected {ollama_url} to be accepted: {error}"));
        }
    }

    #[test]
    fn normalization_rejects_remote_and_misleading_urls() {
        for ollama_url in [
            "https://ollama.example.com",
            "http://192.168.1.8:11434",
            "http://localhost.example.com:11434",
            "http://localhost@ollama.example.com:11434",
        ] {
            let layer = PartialConfig {
                normalization: PartialNormalization {
                    ollama_url: Some(ollama_url.into()),
                    ..Default::default()
                },
                ..Default::default()
            };

            let error = resolve(&[&layer]).unwrap_err().to_string();
            assert!(
                error.contains("local machine"),
                "unexpected error for {ollama_url}: {error}"
            );
        }
    }

    #[test]
    fn normalization_rejects_invalid_or_hostless_urls() {
        for ollama_url in [
            "localhost:11434",
            "file:///tmp/ollama.sock",
            "ftp://localhost:11434",
        ] {
            let layer = PartialConfig {
                normalization: PartialNormalization {
                    ollama_url: Some(ollama_url.into()),
                    ..Default::default()
                },
                ..Default::default()
            };

            let error = resolve(&[&layer]).unwrap_err().to_string();
            assert!(
                error.contains("valid local URL")
                    || error.contains("local machine")
                    || error.contains("HTTP(S) URL"),
                "unexpected error for {ollama_url}: {error}"
            );
        }
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
        let config = resolve(&[&partial]).unwrap();

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
    fn zero_subtitle_limits_are_rejected_during_resolution() {
        for (partial, field) in [
            (
                PartialSubtitles {
                    max_lines: Some(0),
                    ..Default::default()
                },
                "max_lines",
            ),
            (
                PartialSubtitles {
                    max_chars_per_line: Some(0),
                    ..Default::default()
                },
                "max_chars_per_line",
            ),
            (
                PartialSubtitles {
                    max_duration_ms: Some(0),
                    ..Default::default()
                },
                "max_duration_ms",
            ),
        ] {
            let err = resolve(&[&PartialConfig {
                subtitles: partial,
                ..Default::default()
            }])
            .unwrap_err();
            let rendered = err.to_string();
            assert!(rendered.contains(field), "{rendered}");
            assert!(rendered.contains("greater than zero"), "{rendered}");
        }
    }

    #[test]
    fn legacy_reading_speed_key_is_accepted_but_ignored() {
        let partial =
            parse_toml("[subtitles]\nmax_chars_per_second = 17.0\nmax_chars_per_line = 21\n")
                .unwrap();
        let config = resolve(&[&partial]).unwrap();

        assert_eq!(config.subtitles.max_chars_per_line, 21);
        assert!(
            !serde_json::to_string(&config)
                .unwrap()
                .contains("max_chars_per_second")
        );
    }

    #[test]
    fn subtitle_formats_round_trip() {
        assert_eq!(SubtitleFormat::Srt.extension(), "srt");
        assert_eq!(SubtitleFormat::parse("VTT").unwrap(), SubtitleFormat::Vtt);
    }
}
