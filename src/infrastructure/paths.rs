//! XDG locations. Configuration and history live in the user's own directories and
//! are created with owner-only permissions.

use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathError {
    #[error("cannot determine the home directory: neither HOME nor XDG directories are set")]
    NoHome,
    #[error("cannot create {path}: {source}")]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub config_file: PathBuf,
    pub history_file: PathBuf,
    pub log_file: PathBuf,
}

impl AppPaths {
    /// Resolves the XDG locations, honouring `--config` when given.
    pub fn resolve(config_override: Option<PathBuf>) -> Result<Self, PathError> {
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        let state_home = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());

        let config_dir = match config_home {
            Some(dir) => dir.join("omarchy-control-center"),
            None => home_dir()?.join(".config").join("omarchy-control-center"),
        };
        let state_dir = match state_home {
            Some(dir) => dir.join("omarchy-control-center"),
            None => home_dir()?.join(".local").join("state").join("omarchy-control-center"),
        };

        Ok(Self::with_roots(config_dir, state_dir, config_override))
    }

    pub fn with_roots(
        config_dir: PathBuf,
        state_dir: PathBuf,
        config_override: Option<PathBuf>,
    ) -> Self {
        let config_file = config_override
            .unwrap_or_else(|| config_dir.join("config.toml"));
        let history_file = state_dir.join("history.jsonl");
        let log_file = state_dir.join("occ.log");
        Self { config_dir, state_dir, config_file, history_file, log_file }
    }

    /// Creates the directories with owner-only permissions.
    pub fn ensure(&self) -> Result<(), PathError> {
        use std::os::unix::fs::DirBuilderExt;
        for dir in [&self.config_dir, &self.state_dir] {
            if dir.exists() {
                continue;
            }
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|source| PathError::Create { path: dir.clone(), source })?;
        }
        Ok(())
    }
}

fn home_dir() -> Result<PathBuf, PathError> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or(PathError::NoHome)
}

/// Writes a file only its owner can read (mode 0600), replacing it atomically
/// when the platform allows it.
pub fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_override_wins_over_the_default_location() {
        let paths = AppPaths::with_roots(
            PathBuf::from("/tmp/occ-config"),
            PathBuf::from("/tmp/occ-state"),
            Some(PathBuf::from("/tmp/custom.toml")),
        );
        assert_eq!(paths.config_file, PathBuf::from("/tmp/custom.toml"));
        assert_eq!(paths.history_file, PathBuf::from("/tmp/occ-state/history.jsonl"));
    }

    #[test]
    fn private_writes_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("occ-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("secret.txt");
        write_private(&file, "content").expect("write");
        let mode = std::fs::metadata(&file).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read_to_string(&file).expect("read"), "content");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
