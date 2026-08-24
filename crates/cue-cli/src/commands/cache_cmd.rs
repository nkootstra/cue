//! `cue cache` — manage the processing cache directory.

use std::path::PathBuf;

use crate::cli::CacheCommand;
use crate::render::println_line;

/// Cache root: `$CUE_CACHE_DIR`, else `$XDG_CACHE_HOME/cue`, else
/// `~/.cache/cue`.
pub fn cache_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CUE_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .map(std::path::PathBuf::from)
                .map(|h| h.join(".cache"))
        })?;
    Some(base.join("cue"))
}

pub fn run(command: Option<CacheCommand>) -> i32 {
    let Some(dir) = cache_dir() else {
        eprintln!("could not determine a cache directory (set HOME)");
        return 1;
    };

    match command {
        None | Some(CacheCommand::Dir) => {
            println_line(&dir.display().to_string());
            0
        }
        Some(CacheCommand::Clear) => match clear(&dir) {
            Ok(()) => {
                println_line(&format!("cleared {}", dir.display()));
                0
            }
            Err(err) => {
                eprintln!("could not clear cache: {err}");
                1
            }
        },
    }
}

fn clear(dir: &std::path::Path) -> std::io::Result<()> {
    if dir.exists() {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                std::fs::remove_dir_all(path)?;
            } else {
                std::fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}
