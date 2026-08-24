//! `cue cache` — manage the processing cache directory.

use cue_cache::layout::cache_dir;

use crate::cli::CacheCommand;
use crate::render::println_line;

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
