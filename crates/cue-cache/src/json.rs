//! JSON cache entries keyed by content-derived hashes.
//!
//! Every pipeline stage stores its output under a key combining the inputs
//! that could change the result. Reading a valid entry skips the stage;
//! corrupt entries are surfaced so callers can decide to ignore them.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use cue_core::{CueError, Result};

/// A directory of `<key>.json` entries for one stage.
#[derive(Debug, Clone)]
pub struct JsonCache {
    dir: PathBuf,
}

impl JsonCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Load a cached value; `None` when absent.
    ///
    /// Corrupt entries are reported as errors rather than silently ignored:
    /// a truncated file from a crashed run should be visible, and callers
    /// choose whether to proceed without cache.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let path = self.path_for(key);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| {
            CueError::general(format!("could not read cache entry {}", path.display()))
                .because(e.to_string())
        })?;
        serde_json::from_str(&text).map(Some).map_err(|e| {
            CueError::general(format!("cache entry {} is corrupt", path.display()))
                .because(e.to_string())
                .remedy("run `cue cache clear` or delete the entry")
        })
    }

    /// Store a value under `key`, creating the directory as needed.
    pub fn store<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        std::fs::create_dir_all(&self.dir).map_err(|e| {
            CueError::general(format!(
                "could not create cache directory {}",
                self.dir.display()
            ))
            .because(e.to_string())
        })?;
        let json = serde_json::to_string_pretty(value)
            .map_err(|e| CueError::general("serialization failed").because(e.to_string()))?;
        std::fs::write(self.path_for(key), json + "\n")
            .map_err(|e| CueError::general("could not write cache entry").because(e.to_string()))
    }

    /// Remove every entry in this stage's cache directory.
    pub fn clear(&self) -> Result<()> {
        if !self.dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(&self.dir).map_err(|e| {
            CueError::general("could not list cache directory").because(e.to_string())
        })? {
            let path = entry
                .map_err(|e| {
                    CueError::general("could not read cache entry").because(e.to_string())
                })?
                .path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path).map_err(|e| {
                    CueError::general("could not remove cache dir").because(e.to_string())
                })?;
            } else {
                std::fs::remove_file(&path).map_err(|e| {
                    CueError::general("could not remove cache entry").because(e.to_string())
                })?;
            }
        }
        Ok(())
    }

    fn path_for(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Payload {
        value: u32,
        text: String,
    }

    fn payload(v: u32) -> Payload {
        Payload {
            value: v,
            text: "hello".into(),
        }
    }

    #[test]
    fn round_trips_values_by_key() {
        let dir = tempfile::tempdir().unwrap();
        let cache = JsonCache::new(dir.path().join("stage"));

        assert_eq!(cache.get::<Payload>("k1").unwrap(), None);

        cache.store("k1", &payload(7)).unwrap();
        assert_eq!(cache.get::<Payload>("k1").unwrap(), Some(payload(7)));
        // Distinct keys do not collide.
        assert_eq!(cache.get::<Payload>("k2").unwrap(), None);
    }

    #[test]
    fn corrupt_entry_is_an_error_not_none() {
        let dir = tempfile::tempdir().unwrap();
        let cache = JsonCache::new(dir.path());
        std::fs::write(dir.path().join("bad.json"), "{ not json").unwrap();

        let err = cache.get::<Payload>("bad").unwrap_err();
        assert!(err.to_string().contains("corrupt"), "{err}");
    }

    #[test]
    fn missing_directory_is_empty_not_error() {
        let cache = JsonCache::new("/nonexistent/cue-cache-probe");
        assert_eq!(cache.get::<Payload>("x").unwrap(), None);
    }

    #[test]
    fn clear_removes_entries_but_keeps_directory_usable() {
        let dir = tempfile::tempdir().unwrap();
        let cache = JsonCache::new(dir.path());
        cache.store("a", &payload(1)).unwrap();
        cache.clear().unwrap();
        assert_eq!(cache.get::<Payload>("a").unwrap(), None);
        // Still usable afterwards.
        cache.store("b", &payload(2)).unwrap();
        assert_eq!(cache.get::<Payload>("b").unwrap(), Some(payload(2)));
    }
}
