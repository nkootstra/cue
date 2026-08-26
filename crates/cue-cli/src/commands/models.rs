//! `cue models` — transcription and normalization model management.

use cue_core::config::{PartialConfig, load_user_config, resolve};
use cue_llm::OllamaAdmin;

use crate::cli::{ModelsArgs, ModelsCommand};
use crate::render::println_line;

/// The `models` subcommands touch Ollama; run them on the async runtime.
pub async fn run(args: ModelsArgs) -> i32 {
    match args.command {
        None => {
            println_line("Manage transcription and normalization models.");
            println_line("\nUsage:");
            println_line("    cue models list           List models relevant to cue");
            println_line("    cue models check          Verify configured models load");
            println_line("    cue models install s1     Create the S1 model in Ollama");
            0
        }
        Some(ModelsCommand::List) => list().await,
        Some(ModelsCommand::Check) => check().await,
        Some(ModelsCommand::Install(install)) => install_model(&install.model).await,
    }
}

fn ollama_url() -> String {
    let user = load_user_config().unwrap_or_default();
    let config = resolve(&[&PartialConfig::default(), &user]);
    config.normalization.ollama_url
}

async fn list() -> i32 {
    let admin = OllamaAdmin::new(ollama_url());
    match admin.list_models().await {
        Ok(models) => {
            if models.is_empty() {
                println_line("No models found in Ollama.");
                return 0;
            }
            println_line("Models in Ollama:");
            for model in &models {
                let marker = if model.name == cue_normalization::S1_MODEL_NAME
                    || model.name.contains("s1-mini")
                {
                    "  <- normalization"
                } else {
                    ""
                };
                println_line(&format!("  {}{marker}", model.name));
            }
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

async fn check() -> i32 {
    let admin = OllamaAdmin::new(ollama_url());
    match cue_normalization::s1_ready(&admin).await {
        Ok(true) => {
            println_line(&format!(
                "{} is installed and ready for normalization.",
                cue_normalization::S1_MODEL_NAME
            ));
            0
        }
        Ok(false) => {
            println_line(&format!(
                "{} is not installed. Run: cue models install s1",
                cue_normalization::S1_MODEL_NAME
            ));
            1
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

async fn install_model(model: &str) -> i32 {
    if model != "s1" {
        eprintln!("unknown model \"{model}\" — the first supported model is \"s1\"");
        return 2;
    }

    let admin = OllamaAdmin::new(ollama_url());
    println_line(&format!(
        "Installing {} (pulls {} on first use; this can take a while)...",
        cue_normalization::S1_MODEL_NAME,
        cue_normalization::S1_SOURCE_REF
    ));
    match cue_normalization::install_s1(&admin).await {
        Ok(message) => {
            println_line(&message);
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}
