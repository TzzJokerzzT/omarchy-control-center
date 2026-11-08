//! XDG locations. Configuration and history live in the user's own directories and
//! are created with owner-only permissions.

use std::path::PathBuf;

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
            None => home_dir()?
                .join(".local")
                .join("state")
                .join("omarchy-control-center"),
        };

        Ok(Self::with_roots(config_dir, state_dir, config_override))
    }

    pub fn with_roots(
        config_dir: PathBuf,
        state_dir: PathBuf,
        config_override: Option<PathBuf>,
    ) -> Self {
        let config_file = config_override.unwrap_or_else(|| config_dir.join("config.toml"));
        let history_file = state_dir.join("history.jsonl");
        let log_file = state_dir.join("occ.log");
        Self {
            config_dir,
            state_dir,
            config_file,
            history_file,
            log_file,
        }
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
                .map_err(|source| PathError::Create {
                    path: dir.clone(),
                    source,
                })?;
        }
        Ok(())
    }
}

fn home_dir() -> Result<PathBuf, PathError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or(PathError::NoHome)
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
        assert_eq!(
            paths.history_file,
            PathBuf::from("/tmp/occ-state/history.jsonl")
        );
    }
}
