//! Layout of the content-addressed cache.
//!
//! ```text
//! <cache>/
//! └── <media-hash>/
//!     ├── media.json
//!     ├── audio.wav
//!     └── transcription/<config-hash>.json
//! ```

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Cache root: `$CUE_CACHE_DIR`, else `$XDG_CACHE_HOME/cue`, else
/// `~/.cache/cue`.
pub fn cache_dir() -> Option<PathBuf> {
    cache_dir_from(|key| std::env::var_os(key))
}

/// Pure resolution logic with an injected environment lookup.
pub fn cache_dir_from(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(dir) = env("CUE_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = env("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| {
        env("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .map(|h| h.join(".cache"))
    })?;
    Some(base.join("cue"))
}

/// Per-media directory keyed by the media file's content hash.
pub fn media_dir(root: &Path, media_hash: &str) -> PathBuf {
    root.join(media_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_override_wins() {
        let dir = cache_dir_from(|key| match key {
            "CUE_CACHE_DIR" => Some("/override".into()),
            _ => None,
        });
        assert_eq!(dir, Some(PathBuf::from("/override")));
    }

    #[test]
    fn xdg_cache_home_is_respected() {
        let dir = cache_dir_from(|key| match key {
            "XDG_CACHE_HOME" => Some("/xdg".into()),
            _ => None,
        });
        assert_eq!(dir, Some(PathBuf::from("/xdg/cue")));
    }

    #[test]
    fn home_falls_back_to_dotcache() {
        let dir = cache_dir_from(|key| match key {
            "HOME" => Some("/Users/someone".into()),
            _ => None,
        });
        assert_eq!(dir, Some(PathBuf::from("/Users/someone/.cache/cue")));
    }

    #[test]
    fn empty_home_is_ignored() {
        let dir = cache_dir_from(|key| match key {
            "HOME" => Some("".into()),
            _ => None,
        });
        assert_eq!(dir, None);
    }

    #[test]
    fn media_dirs_nest_under_root() {
        let dir = media_dir(Path::new("/cache"), "abc123");
        assert_eq!(dir, PathBuf::from("/cache/abc123"));
    }
}
