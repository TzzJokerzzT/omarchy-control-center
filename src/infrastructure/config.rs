//! Configuration file. Deliberately small: a read-only switch, extra search paths
//! and a command deadline. An unreadable or invalid file is reported and the
//! defaults are used, so a typo never prevents the inventory from opening.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid configuration in {path}: {detail}")]
    Parse { path: PathBuf, detail: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Explore the inventory without exposing destructive actions.
    pub read_only: bool,
    /// Extra directories prepended to the executable search path, in order.
    pub extra_search_paths: Vec<PathBuf>,
    /// Theme id to start with. Overrides the one chosen in the theme selector.
    pub theme: Option<String>,
    /// Ask the release API at startup whether a newer version exists. Off by default: a
    /// program that contacts a server should be asked to, not assumed to.
    pub check_for_updates: bool,
}

impl Config {
    /// Loads the file when it exists. A missing file is not an error.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text).map_err(|detail| ConfigError::Parse {
            path: path.to_path_buf(),
            detail,
        })
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str::<Self>(text).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_conservative() {
        let config = Config::default();
        assert!(!config.read_only);
        assert!(config.extra_search_paths.is_empty());
    }

    #[test]
    fn parses_a_partial_file_and_keeps_defaults_for_the_rest() {
        let config = Config::parse("read_only = true\n").expect("parse");
        assert!(config.read_only);
        assert!(config.extra_search_paths.is_empty());
    }

    #[test]
    fn an_unknown_key_is_reported_instead_of_silently_ignored() {
        let error = Config::parse("reed_only = true\n").unwrap_err();
        assert!(error.contains("reed_only"), "unexpected error: {error}");
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let path = std::env::temp_dir().join("occ-config-absent/config.toml");
        assert!(Config::load(&path).is_ok());
    }
}
