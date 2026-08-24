//! Content addressing: BLAKE3 hashes over file bytes.
//!
//! Cache keys always derive from content plus configuration, never from
//! paths or timestamps, so moving files around never invalidates work.

use std::path::Path;

use blake3::Hasher;
use cue_core::{CueError, Result};

/// BLAKE3 hex digest of a file's contents.
pub fn file_hash(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|e| {
        CueError::general(format!("could not open {} for hashing", path.display()))
            .because(e.to_string())
    })?;
    let mut hasher = Hasher::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| {
        CueError::general("could not read file while hashing").because(e.to_string())
    })?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// BLAKE3 hex digest of arbitrary bytes.
pub fn bytes_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_content_same_hash_different_paths() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        let b = dir.path().join("nested").join("b.bin");
        std::fs::create_dir_all(b.parent().unwrap()).unwrap();
        std::fs::write(&a, b"identical payload").unwrap();
        std::fs::write(&b, b"identical payload").unwrap();

        assert_eq!(file_hash(&a).unwrap(), file_hash(&b).unwrap());
    }

    #[test]
    fn different_content_different_hash() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, b"one").unwrap();
        std::fs::write(&b, b"two").unwrap();
        assert_ne!(file_hash(&a).unwrap(), file_hash(&b).unwrap());
    }

    #[test]
    fn missing_file_is_an_error_with_context() {
        let err = file_hash(Path::new("/nonexistent/hash-me")).unwrap_err();
        assert!(err.to_string().contains("could not open"), "{err}");
    }

    #[test]
    fn byte_hashes_are_stable_hex() {
        let h = bytes_hash(b"stable");
        assert_eq!(h.len(), 64);
        assert_eq!(bytes_hash(b"stable"), h);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
