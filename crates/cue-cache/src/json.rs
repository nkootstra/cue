//! JSON cache entries keyed by content-derived hashes.
//!
//! Every pipeline stage stores its output under a key combining the inputs
//! that could change the result. Reading a valid entry skips the stage;
//! corrupt entries are surfaced so callers can decide to ignore them.

use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde::de::DeserializeOwned;

use cue_core::{CueError, Result};

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A directory of JSON entries addressed by hashed logical keys.
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
    pub fn get<T: DeserializeOwned>(&self, key: &impl Serialize) -> Result<Option<T>> {
        let path = self.path_for(key)?;
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(CueError::general(format!(
                    "could not read cache entry {}",
                    path.display()
                ))
                .because(error.to_string()));
            }
        };
        serde_json::from_slice(&bytes).map(Some).map_err(|e| {
            CueError::general(format!("cache entry {} is corrupt", path.display()))
                .because(e.to_string())
                .remedy("run `cue cache clear` or delete the entry")
        })
    }

    /// Store a value under `key`, creating the directory as needed.
    pub fn store<T: Serialize>(&self, key: &impl Serialize, value: &T) -> Result<()> {
        let path = self.path_for(key)?;
        let mut json = serde_json::to_vec_pretty(value)
            .map_err(|e| CueError::general("serialization failed").because(e.to_string()))?;
        json.push(b'\n');

        std::fs::create_dir_all(&self.dir).map_err(|e| {
            CueError::general(format!(
                "could not create cache directory {}",
                self.dir.display()
            ))
            .because(e.to_string())
        })?;

        let (temp_path, mut temp_file) = self.create_temp_file(&path)?;
        if let Err(error) = temp_file
            .write_all(&json)
            .and_then(|()| temp_file.sync_all())
        {
            drop(temp_file);
            let _ = std::fs::remove_file(&temp_path);
            return Err(CueError::general("could not write cache entry").because(error.to_string()));
        }
        drop(temp_file);

        match std::fs::rename(&temp_path, &path) {
            Ok(()) => Ok(()),
            // Some platforms do not replace an entry created by a competing
            // writer. That writer has already published a complete value for
            // this logical key, so the store is still successful.
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let _ = std::fs::remove_file(temp_path);
                Ok(())
            }
            Err(error) => {
                let _ = std::fs::remove_file(temp_path);
                Err(CueError::general("could not publish cache entry").because(error.to_string()))
            }
        }
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

    fn path_for(&self, key: &impl Serialize) -> Result<PathBuf> {
        let key_json = serde_json::to_value(key).map_err(|error| {
            CueError::general("could not serialize cache key").because(error.to_string())
        })?;
        let key_json = serde_json::to_vec(&key_json).map_err(|error| {
            CueError::general("could not serialize canonical cache key").because(error.to_string())
        })?;
        let digest = crate::bytes_hash(&key_json);
        Ok(self.dir.join(format!("{digest}.json")))
    }

    fn create_temp_file(&self, path: &Path) -> Result<(PathBuf, std::fs::File)> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| CueError::general("could not construct cache temporary file name"))?;

        loop {
            let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let temp_path = self.dir.join(format!(
                ".{file_name}.{}.{}.tmp",
                std::process::id(),
                sequence
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
            {
                Ok(file) => return Ok((temp_path, file)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(CueError::general("could not create cache temporary file")
                        .because(error.to_string()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::ser::SerializeMap as _;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct Payload {
        value: u32,
        text: String,
    }

    #[derive(Serialize)]
    struct LogicalKey<'a> {
        version: u8,
        left: &'a str,
        right: &'a str,
    }

    struct DifferentlyOrderedKey {
        reverse: bool,
    }

    impl Serialize for DifferentlyOrderedKey {
        fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            let mut map = serializer.serialize_map(Some(2))?;
            if self.reverse {
                map.serialize_entry("right", "b")?;
                map.serialize_entry("left", "a")?;
            } else {
                map.serialize_entry("left", "a")?;
                map.serialize_entry("right", "b")?;
            }
            map.end()
        }
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

        let key = LogicalKey {
            version: 1,
            left: "k1",
            right: "value",
        };
        let other_key = LogicalKey {
            version: 1,
            left: "k2",
            right: "value",
        };

        assert_eq!(cache.get::<Payload>(&key).unwrap(), None);

        cache.store(&key, &payload(7)).unwrap();
        assert_eq!(cache.get::<Payload>(&key).unwrap(), Some(payload(7)));
        // Distinct keys do not collide.
        assert_eq!(cache.get::<Payload>(&other_key).unwrap(), None);
    }

    #[test]
    fn structured_keys_do_not_have_delimiter_collisions_or_unsafe_paths() {
        let dir = tempfile::tempdir().unwrap();
        let cache = JsonCache::new(dir.path());
        let formerly_colliding_a = LogicalKey {
            version: 1,
            left: "a-b",
            right: "c",
        };
        let formerly_colliding_b = LogicalKey {
            version: 1,
            left: "a",
            right: "b-c",
        };
        let unsafe_shaped = LogicalKey {
            version: 1,
            left: "../outside",
            right: "a/b",
        };

        cache.store(&formerly_colliding_a, &payload(1)).unwrap();
        cache.store(&formerly_colliding_b, &payload(2)).unwrap();
        cache.store(&unsafe_shaped, &payload(3)).unwrap();

        assert_eq!(cache.get(&formerly_colliding_a).unwrap(), Some(payload(1)));
        assert_eq!(cache.get(&formerly_colliding_b).unwrap(), Some(payload(2)));
        assert_eq!(cache.get(&unsafe_shaped).unwrap(), Some(payload(3)));

        let names = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 3);
        assert!(names.iter().all(|name| {
            name.len() == 69
                && name.ends_with(".json")
                && name[..64].bytes().all(|byte| byte.is_ascii_hexdigit())
        }));
    }

    #[test]
    fn key_identity_is_deterministic_and_sensitive_to_each_field() {
        let cache = JsonCache::new("unused");
        let key = LogicalKey {
            version: 1,
            left: "left",
            right: "right",
        };
        let changed_version = LogicalKey { version: 2, ..key };
        let changed_field = LogicalKey {
            version: 1,
            left: "left",
            right: "different",
        };

        assert_eq!(cache.path_for(&key).unwrap(), cache.path_for(&key).unwrap());
        assert_ne!(
            cache.path_for(&key).unwrap(),
            cache.path_for(&changed_version).unwrap()
        );
        assert_ne!(
            cache.path_for(&key).unwrap(),
            cache.path_for(&changed_field).unwrap()
        );
        assert_eq!(
            cache
                .path_for(&DifferentlyOrderedKey { reverse: false })
                .unwrap(),
            cache
                .path_for(&DifferentlyOrderedKey { reverse: true })
                .unwrap()
        );
    }

    #[test]
    fn corrupt_entry_is_an_error_not_none() {
        let dir = tempfile::tempdir().unwrap();
        let cache = JsonCache::new(dir.path());
        let key = LogicalKey {
            version: 1,
            left: "bad",
            right: "value",
        };
        std::fs::write(cache.path_for(&key).unwrap(), "{ not json").unwrap();

        let err = cache.get::<Payload>(&key).unwrap_err();
        assert!(err.to_string().contains("corrupt"), "{err}");
    }

    #[test]
    fn missing_directory_is_empty_not_error() {
        let cache = JsonCache::new("/nonexistent/cue-cache-probe");
        assert_eq!(cache.get::<Payload>(&"x").unwrap(), None);
    }

    #[test]
    fn clear_removes_entries_but_keeps_directory_usable() {
        let dir = tempfile::tempdir().unwrap();
        let cache = JsonCache::new(dir.path());
        cache.store(&"a", &payload(1)).unwrap();
        cache.clear().unwrap();
        assert_eq!(cache.get::<Payload>(&"a").unwrap(), None);
        // Still usable afterwards.
        cache.store(&"b", &payload(2)).unwrap();
        assert_eq!(cache.get::<Payload>(&"b").unwrap(), Some(payload(2)));
    }

    #[test]
    fn concurrent_writers_never_expose_partial_json() {
        let dir = tempfile::tempdir().unwrap();
        let cache = JsonCache::new(dir.path());
        let key = LogicalKey {
            version: 1,
            left: "shared",
            right: "entry",
        };
        cache.store(&key, &payload(0)).unwrap();

        std::thread::scope(|scope| {
            for value in 1..=8 {
                let cache = cache.clone();
                let key = &key;
                scope.spawn(move || {
                    for _ in 0..50 {
                        cache.store(key, &payload(value)).unwrap();
                    }
                });
            }

            for _ in 0..1_000 {
                assert!(cache.get::<Payload>(&key).unwrap().is_some());
            }
        });

        assert!(cache.get::<Payload>(&key).unwrap().is_some());
        assert_eq!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(|entry| {
                    let name = entry.ok()?.file_name().into_string().ok()?;
                    name.contains(".tmp").then_some(name)
                })
                .count(),
            0
        );
    }
}
