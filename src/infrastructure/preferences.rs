//! Choices the interface remembers between runs.
//!
//! Deliberately separate from `config.toml`: that file belongs to the user and carries
//! their comments, so rewriting it from a keystroke would destroy their work. This file
//! holds only what the interface itself chose, and a `theme` set in `config.toml` always
//! wins over it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    /// Theme id chosen in the selector, e.g. `matrix`.
    pub theme: Option<String>,
}

impl Preferences {
    /// Loads the file when it exists. A missing or unreadable file is an empty set of
    /// preferences, never an error the caller has to handle.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match toml::from_str::<Self>(&text) {
            Ok(preferences) => preferences,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "ignoring unreadable preferences");
                Self::default()
            }
        }
    }

    /// Writes the file owner-only, atomically: a crash mid-write leaves the previous
    /// preferences intact instead of a truncated file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string(self).unwrap_or_default();
        let temporary: PathBuf = path.with_extension("tmp");
        {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        std::fs::rename(&temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("occ-prefs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir.join("preferences.toml")
    }

    #[test]
    fn a_missing_file_is_empty_preferences() {
        let path = sandbox("missing");
        assert_eq!(Preferences::load(&path), Preferences::default());
    }

    #[test]
    fn saves_and_reads_the_chosen_theme() {
        let path = sandbox("roundtrip");
        let preferences = Preferences {
            theme: Some("matrix".to_string()),
        };
        preferences.save(&path).expect("save");
        assert_eq!(Preferences::load(&path), preferences);
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn the_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = sandbox("mode");
        Preferences {
            theme: Some("zelda".to_string()),
        }
        .save(&path)
        .expect("save");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn a_corrupt_file_falls_back_to_empty_instead_of_failing() {
        let path = sandbox("corrupt");
        std::fs::write(&path, "this is not toml {{{").expect("seed");
        assert_eq!(Preferences::load(&path), Preferences::default());
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn an_unknown_key_is_not_a_reason_to_fail_on_load() {
        let path = sandbox("unknown");
        std::fs::write(&path, "theme = \"matrix\"\nsomething_else = 1\n").expect("seed");
        assert_eq!(Preferences::load(&path), Preferences::default());
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }
}
