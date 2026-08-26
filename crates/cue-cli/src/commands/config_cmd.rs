//! `cue config` — show resolved configuration.

use cue_core::config::{PartialConfig, load_user_config, resolve, user_config_path};

use crate::cli::ConfigArgs;
use crate::render::println_line;

pub fn run(args: ConfigArgs) -> i32 {
    if args.path {
        match user_config_path() {
            Some(path) => println_line(&path.display().to_string()),
            None => {
                eprintln!("could not determine a configuration directory (set HOME)");
                return 1;
            }
        }
        return 0;
    }

    let path = user_config_path();
    match load_user_config() {
        Ok(user) => {
            let resolved = match resolve(&[&PartialConfig::default(), &user]) {
                Ok(resolved) => resolved,
                Err(err) => {
                    eprintln!("{err}");
                    return 1;
                }
            };
            match serde_json::to_string_pretty(&resolved) {
                Ok(json) => println_line(&json),
                Err(e) => {
                    eprintln!("could not serialize configuration: {e}");
                    return 1;
                }
            }
            if let Some(path) = path {
                if path.exists() {
                    println_line(&format!("\n# loaded from {}", path.display()));
                } else {
                    println_line(&format!(
                        "\n# defaults only — no file at {}",
                        path.display()
                    ));
                }
            }
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}
