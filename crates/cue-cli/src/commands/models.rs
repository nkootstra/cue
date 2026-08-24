//! `cue models` — transcription and normalization model management.
//!
//! Real Ollama integration lands with the normalization milestone; these
//! commands explain their status honestly rather than failing silently.

use crate::cli::{ModelsArgs, ModelsCommand};
use crate::render::println_line;

pub fn run(args: ModelsArgs) -> i32 {
    match args.command {
        None => {
            println_line("Manage transcription and normalization models.");
            println_line("\nUsage:");
            println_line("    cue models list           List models relevant to cue");
            println_line("    cue models check          Verify configured models load");
            println_line("    cue models install s1     Create the S1 model in Ollama");
            0
        }
        Some(ModelsCommand::List) => list(),
        Some(ModelsCommand::Check) => check(),
        Some(ModelsCommand::Install(install)) => install_model(&install.model),
    }
}

fn not_ready(what: &str) -> i32 {
    println_line(&format!("{what} is not implemented yet."));
    println_line("Model management arrives together with Ollama integration.");
    1
}

fn list() -> i32 {
    not_ready("model listing")
}

fn check() -> i32 {
    not_ready("model checking")
}

fn install_model(model: &str) -> i32 {
    if model != "s1" {
        eprintln!("unknown model \"{model}\" — the first supported model is \"s1\"");
        return 2;
    }
    not_ready("S1 installation")
}
