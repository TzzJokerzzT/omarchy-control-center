//! Reclaiming the space an unmanaged leftover occupies.
//!
//! "Orphan" is not one thing, and the difference decides what may happen to it:
//!
//! * a package the package manager no longer needs is **owned by that manager**, so it
//!   is removed by asking the manager, one package at a time;
//! * a leftover directory or file that no manager owns — a `.bak` plugin directory, a
//!   Hyprland backup copy — can only be dealt with by moving it out of the way, and that
//!   is what this module does, with the path verified rather than trusted.
//!
//! Nothing here deletes by matching a name. A reclaim target must be a direct child of a
//! root the adapter owns, must not be a symlink, and is **moved** into a quarantine area
//! that records where it came from, so the action has a real way back. Deleting for good
//! is a separate, explicit step over entries the ledger already knows.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReclaimDisposition {
    /// Move into the quarantine area; the entry can be restored from there.
    Quarantine,
}

/// One path a reclaim would take, as planned by an adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReclaimTarget {
    pub path: PathBuf,
    /// The directory this target must be a direct child of. The adapter declares it; the
    /// reclaimer verifies it.
    pub root: PathBuf,
    /// Bytes occupied, measured when the plan was built. A different size at execution
    /// time is a changed plan.
    pub size: u64,
    pub disposition: ReclaimDisposition,
}

impl ReclaimTarget {
    pub fn quarantine(path: impl Into<PathBuf>, root: impl Into<PathBuf>, size: u64) -> Self {
        Self {
            path: path.into(),
            root: root.into(),
            size,
            disposition: ReclaimDisposition::Quarantine,
        }
    }
}

/// A path that was moved into the quarantine area.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quarantined {
    /// Where it was before, and where a restore would put it back.
    pub original: PathBuf,
    /// Where it lives now.
    pub location: PathBuf,
    /// The root it was a child of, kept so a restore can re-verify it.
    pub root: PathBuf,
    pub size: u64,
    /// Batch stamp: everything moved in one reclaim shares it.
    pub stamp: u64,
    pub moved_at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReclaimError {
    #[error("{path} is a symbolic link; nothing is ever moved through a link")]
    Symlink { path: PathBuf },

    #[error("{path} does not exist")]
    Missing { path: PathBuf },

    #[error("{path} is not a direct child of {root}")]
    OutsideRoot { path: PathBuf, root: PathBuf },

    #[error("refusing to reclaim {path}: {reason}")]
    Unsafe { path: PathBuf, reason: String },

    #[error("cannot move {path} into quarantine: {message}")]
    Move { path: PathBuf, message: String },

    #[error("cannot record the quarantine entry: {message}")]
    Ledger { message: String },

    #[error("cannot restore {path}: {message}")]
    Restore { path: PathBuf, message: String },

    #[error("cannot purge {path}: {message}")]
    Purge { path: PathBuf, message: String },
}

/// Bytes under `path`, with a file cap so a pathological tree cannot stall the interface.
pub fn path_size(path: &Path) -> u64 {
    const MAX_ENTRIES: usize = 20_000;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return 0,
    };
    if metadata.is_file() {
        return metadata.len();
    }
    if !metadata.is_dir() {
        return 0;
    }

    let mut total = 0;
    let mut visited = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_ENTRIES {
                return total;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                total += metadata.len();
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_a_tree_and_a_single_file() {
        let dir = std::env::temp_dir().join(format!("occ-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("nested")).expect("dir");
        std::fs::write(dir.join("a.txt"), vec![b'x'; 1000]).expect("a");
        std::fs::write(dir.join("nested/b.txt"), vec![b'x'; 500]).expect("b");

        assert_eq!(path_size(&dir), 1500);
        assert_eq!(path_size(&dir.join("a.txt")), 1000);
        assert_eq!(path_size(&dir.join("absent")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
