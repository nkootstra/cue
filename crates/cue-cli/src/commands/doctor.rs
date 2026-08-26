//! `cue doctor` — inspect the local environment, optionally fixing it.
//!
//! Required tools gate the exit code; optional integrations (Ollama, S1,
//! an LLM gateway) are reported without failing local-only setups.

use cue_llm::OllamaAdmin;
use cue_media::{ToolReport, check_environment};

use crate::cli::DoctorArgs;
use crate::render::{println_line, tool_line};

/// Run environment checks and render them. Returns the process exit code.
pub async fn run(args: DoctorArgs, config: &cue_core::Config) -> i32 {
    println_line("Checking local environment...\n");
    let env = check_environment().await;

    println_line("Required:");
    for report in &env.reports {
        println_line(&tool_line(report));
    }

    println_line("\nOptional (not required for local transcription):");
    for line in integration_lines(config).await {
        println_line(&line);
    }
    println_line("\nLocal transcription and subtitles work without any of these.");

    let fixed_ok = if args.fix {
        Some(try_fix(&env).await)
    } else {
        None
    };

    match (env.all_ok(), fixed_ok) {
        (true, _) => {
            println_line("\nAll required tools are available.");
            0
        }
        (false, Some(true)) => {
            println_line("\nTranscription environment is ready. Re-run `cue doctor`");
            println_line("for a full status.");
            0
        }
        (_, _) => {
            println_line("\nSome required tools are missing. Install FFmpeg (and");
            println_line("Python 3.10+), or run `cue doctor --fix` to set up the Python");
            println_line("transcription environment.");
            1
        }
    }
}

/// Status lines for the optional integrations, in display order.
pub async fn integration_lines(config: &cue_core::Config) -> Vec<String> {
    let mut lines = Vec::new();

    // Ollama reachability and S1 presence share one probe.
    let admin = OllamaAdmin::new(&config.normalization.ollama_url);
    match admin.list_models().await {
        Ok(models) => {
            lines.push(format!(
                "{:<10} ok       {}",
                "Ollama", config.normalization.ollama_url
            ));
            lines.push(s1_line(
                cue_normalization::s1_ready_in(&models),
                cue_normalization::S1_MODEL_NAME,
            ));
        }
        Err(reason) => {
            lines.push(format!(
                "{:<10} error    probe failed at {}",
                "Ollama", config.normalization.ollama_url
            ));
            tracing::debug!(reason = %reason, "ollama probe failed");
            lines.push(format!(
                "{:<10} unknown  Ollama probe failed; model presence was not checked",
                "S1"
            ));
        }
    }

    lines.push(llm_line(config.llm.as_ref()));
    lines
}

fn s1_line(installed: bool, model_name: &str) -> String {
    if installed {
        format!("{:<10} ok       {model_name} ready", "S1")
    } else {
        format!(
            "{:<10} not configured  run `cue models install s1` for cleaned transcripts",
            "S1"
        )
    }
}

fn llm_line(llm: Option<&cue_core::config::LlmConfig>) -> String {
    match llm {
        None => format!(
            "{:<10} not configured  no gateway — summaries/descriptions skipped",
            "LLM"
        ),
        Some(config) => {
            let key_state = if config.api_key().is_some() {
                "key set".to_string()
            } else {
                format!("env var {} is unset", config.api_key_env)
            };
            format!(
                "{:<10} ok       {} ({}, {key_state})",
                "LLM", config.base_url, config.model
            )
        }
    }
}

/// Attempt to provision the Python transcription environment.
///
/// Returns true when the environment is usable afterwards.
async fn try_fix(env: &cue_media::checks::Environment) -> bool {
    let Some(python) = python_for_provisioning(env) else {
        println_line("\n--fix: no Python found; cannot provision the transcription");
        println_line("environment. Install Python 3.10+ first.");
        return false;
    };

    let Some(data_dir) = cue_transcription::env::data_dir() else {
        println_line("\n--fix: could not determine cue's data directory (set HOME).");
        return false;
    };

    let venv_dir = cue_transcription::provision::venv_dir(&data_dir);
    println_line("\n--fix: provisioning the transcription environment...");
    println_line(&format!("  venv: {}", venv_dir.display()));
    println_line("  installing pinned faster-whisper dependencies");

    match cue_transcription::provision::provision(&data_dir, &python, false).await {
        Ok(cue_transcription::provision::ProvisionAction::AlreadyProvisioned) => {
            println_line("  already provisioned — nothing to do");
            true
        }
        Ok(cue_transcription::provision::ProvisionAction::Created) => {
            println_line("  done");
            true
        }
        Err(err) => {
            eprintln!("{err}");
            false
        }
    }
}

/// Prefer the checked system python for creating the venv.
fn python_for_provisioning(env: &cue_media::checks::Environment) -> Option<std::path::PathBuf> {
    let report: &ToolReport = env.python()?;
    match &report.status {
        cue_media::ToolStatus::Available { path, .. } => Some(std::path::PathBuf::from(path)),
        cue_media::ToolStatus::Missing | cue_media::ToolStatus::Error(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn integrations_derive_ollama_and_s1_from_one_probe() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                r#"{{"models":[{{"name":"{}"}}]}}"#,
                cue_normalization::S1_MODEL_NAME
            )))
            .expect(1)
            .mount(&server)
            .await;

        let mut config = cue_core::Config::default();
        config.normalization.ollama_url = server.uri();
        let lines = integration_lines(&config).await;

        assert!(
            lines
                .iter()
                .any(|line| line.contains("Ollama") && line.contains("ok"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("S1") && line.contains("ready"))
        );
    }

    #[test]
    fn s1_line_distinguishes_installed_from_not_configured() {
        let ok = s1_line(true, "cue-s1-mini");
        assert!(ok.contains("ready"), "{ok}");

        let not_configured = s1_line(false, "cue-s1-mini");
        assert!(
            not_configured.contains("not configured"),
            "{not_configured}"
        );
        assert!(
            not_configured.contains("models install"),
            "{not_configured}"
        );
    }

    #[test]
    fn llm_line_reports_not_configured_when_unconfigured() {
        let line = llm_line(None);
        assert!(line.contains("not configured"), "{line}");
        assert!(line.contains("summaries/descriptions"), "{line}");
    }

    #[test]
    fn llm_line_reports_key_state() {
        let configured = cue_core::config::LlmConfig {
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "test-model".into(),
            api_key_env: "CURE_DEFINITELY_UNSET_VAR_12345".into(),
        };
        let line = llm_line(Some(&configured));
        assert!(line.contains("https://openrouter.ai/api/v1"), "{line}");
        assert!(
            line.contains("CURE_DEFINITELY_UNSET_VAR_12345 is unset"),
            "{line}"
        );
    }

    #[test]
    fn provisioning_uses_only_the_checked_supported_python() {
        let available = cue_media::checks::Environment {
            reports: vec![ToolReport {
                name: "Python".into(),
                status: cue_media::ToolStatus::Available {
                    path: "/opt/python3".into(),
                    version: "Python 3.10.0".into(),
                },
            }],
        };
        assert_eq!(
            python_for_provisioning(&available),
            Some(std::path::PathBuf::from("/opt/python3"))
        );

        let rejected = cue_media::checks::Environment {
            reports: vec![ToolReport {
                name: "Python".into(),
                status: cue_media::ToolStatus::Error("Python 3.9 is unsupported".into()),
            }],
        };
        assert_eq!(python_for_provisioning(&rejected), None);
    }
}
