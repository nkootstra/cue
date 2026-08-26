//! Platform path resolution shared by cue's runtime subsystems.

use std::ffi::OsString;
use std::path::PathBuf;

/// Cue's data directory: `$CUE_DATA_DIR`, else `$XDG_DATA_HOME/cue`, else
/// `~/.local/share/cue`.
pub fn data_dir() -> Option<PathBuf> {
    data_dir_from(
        std::env::var_os("CUE_DATA_DIR"),
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
    )
}

fn data_dir_from(
    cue_data_dir: Option<OsString>,
    xdg_data_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    if let Some(dir) = cue_data_dir.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir));
    }

    let base = xdg_data_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .map(|home| home.join(".local/share"))
        })?;
    Some(base.join("cue"))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::data_dir_from;

    #[test]
    fn cue_data_dir_has_highest_precedence() {
        assert_eq!(
            data_dir_from(
                Some(OsString::from("/custom cue data")),
                Some(OsString::from("/xdg")),
                Some(OsString::from("/home")),
            ),
            Some(PathBuf::from("/custom cue data"))
        );
    }

    #[test]
    fn xdg_data_home_precedes_home() {
        assert_eq!(
            data_dir_from(
                None,
                Some(OsString::from("/xdg data")),
                Some(OsString::from("/home")),
            ),
            Some(PathBuf::from("/xdg data/cue"))
        );
    }

    #[test]
    fn empty_cue_data_dir_falls_back_to_xdg_data_home() {
        assert_eq!(
            data_dir_from(
                Some(OsString::new()),
                Some(OsString::from("/xdg data")),
                Some(OsString::from("/home")),
            ),
            Some(PathBuf::from("/xdg data/cue"))
        );
    }

    #[test]
    fn empty_xdg_data_home_falls_back_to_home() {
        assert_eq!(
            data_dir_from(
                None,
                Some(OsString::new()),
                Some(OsString::from("/home/user name")),
            ),
            Some(PathBuf::from("/home/user name/.local/share/cue"))
        );
    }

    #[test]
    fn home_uses_local_share_fallback() {
        assert_eq!(
            data_dir_from(None, None, Some(OsString::from("/home/user name"))),
            Some(PathBuf::from("/home/user name/.local/share/cue"))
        );
    }

    #[test]
    fn missing_or_empty_home_has_no_fallback() {
        assert_eq!(data_dir_from(None, None, None), None);
        assert_eq!(data_dir_from(None, None, Some(OsString::new())), None);
    }
}
