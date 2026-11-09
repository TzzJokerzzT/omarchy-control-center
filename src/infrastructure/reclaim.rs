//! The quarantine area: where a reclaim actually puts things, and the only place that
//! deletes them for good.
//!
//! The rules that make this safe to run from an interface:
//!
//! * a target must be a **direct child** of a root the adapter declared, and the
//!   comparison is done on canonical paths, so `..` or a symlinked parent cannot move the
//!   boundary;
//! * a symbolic link is refused outright — following one is how a clean-up deletes
//!   something it was never shown;
//! * nothing is deleted: entries are **moved**, and the move is recorded in a ledger with
//!   the original path, so a restore is a real operation and not a promise;
//! * a permanent delete only accepts paths that the ledger already knows, which means it
//!   can never be talked into deleting an arbitrary path.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::domain::{Quarantined, ReclaimError, ReclaimTarget, path_size};

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

pub trait Reclaimer: Send + Sync {
    /// Moves one planned target into the quarantine area.
    fn quarantine(&self, target: &ReclaimTarget, stamp: u64) -> Result<Quarantined, ReclaimError>;

    /// Everything already in quarantine, newest first.
    fn entries(&self) -> Result<Vec<Quarantined>, ReclaimError>;

    /// Puts one entry back where it came from.
    fn restore(&self, entry: &Quarantined) -> Result<(), ReclaimError>;

    /// Deletes one entry permanently. Only an entry this reclaimer recorded may be purged.
    fn purge(&self, entry: &Quarantined) -> Result<(), ReclaimError>;
}

pub struct FileReclaimer {
    quarantine_root: PathBuf,
    ledger: PathBuf,
    write_lock: Mutex<()>,
}

impl FileReclaimer {
    pub fn new(state_dir: impl AsRef<Path>) -> Self {
        let quarantine_root = state_dir.as_ref().join("quarantine");
        let ledger = quarantine_root.join("index.jsonl");
        Self {
            quarantine_root,
            ledger,
            write_lock: Mutex::new(()),
        }
    }

    /// Where reclaims land.
    #[cfg(test)]
    pub fn quarantine_root(&self) -> &Path {
        &self.quarantine_root
    }

    fn read_ledger(&self) -> Result<Vec<Quarantined>, ReclaimError> {
        if !self.ledger.exists() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&self.ledger).map_err(|error| ReclaimError::Ledger {
            message: error.to_string(),
        })?;
        let mut entries = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Quarantined>(line) {
                Ok(entry) => entries.push(entry),
                Err(error) => tracing::warn!(%error, "skipping an unreadable quarantine line"),
            }
        }
        entries.reverse();
        Ok(entries)
    }

    fn append_ledger(&self, entry: &Quarantined) -> Result<(), ReclaimError> {
        use std::os::unix::fs::OpenOptionsExt;
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        if let Some(parent) = self.ledger.parent() {
            std::fs::create_dir_all(parent).map_err(|error| ReclaimError::Ledger {
                message: error.to_string(),
            })?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.ledger)
            .map_err(|error| ReclaimError::Ledger {
                message: error.to_string(),
            })?;
        let line = serde_json::to_string(entry).map_err(|error| ReclaimError::Ledger {
            message: error.to_string(),
        })?;
        writeln!(file, "{line}").map_err(|error| ReclaimError::Ledger {
            message: error.to_string(),
        })
    }

    /// The verification that decides whether a planned target may be moved at all.
    fn verify(&self, target: &ReclaimTarget) -> Result<(PathBuf, PathBuf), ReclaimError> {
        let metadata =
            std::fs::symlink_metadata(&target.path).map_err(|_| ReclaimError::Missing {
                path: target.path.clone(),
            })?;
        if metadata.file_type().is_symlink() {
            return Err(ReclaimError::Symlink {
                path: target.path.clone(),
            });
        }

        // Canonicalise both sides: `..`, a symlinked parent or a relative root would
        // otherwise let the path escape the boundary the adapter declared.
        let canonical_target =
            std::fs::canonicalize(&target.path).map_err(|_| ReclaimError::Missing {
                path: target.path.clone(),
            })?;
        let canonical_root =
            std::fs::canonicalize(&target.root).map_err(|_| ReclaimError::Unsafe {
                path: target.path.clone(),
                reason: format!(
                    "its root {} is not a readable directory",
                    target.root.display()
                ),
            })?;

        if canonical_target == canonical_root {
            return Err(ReclaimError::Unsafe {
                path: target.path.clone(),
                reason: "the target is the root itself".to_string(),
            });
        }
        if canonical_target.parent() != Some(canonical_root.as_path()) {
            return Err(ReclaimError::OutsideRoot {
                path: target.path.clone(),
                root: target.root.clone(),
            });
        }
        if is_forbidden_root(&canonical_target) {
            return Err(ReclaimError::Unsafe {
                path: target.path.clone(),
                reason: "that is a system or home directory".to_string(),
            });
        }
        Ok((canonical_target, canonical_root))
    }
}

fn is_forbidden_root(path: &Path) -> bool {
    if path.parent().is_none() {
        return true;
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && let Ok(home) = std::fs::canonicalize(home)
        && (path == home || home.starts_with(path))
    {
        return true;
    }
    matches!(
        path.to_str(),
        Some("/") | Some("/home") | Some("/usr") | Some("/etc") | Some("/var") | Some("/boot")
    )
}

impl Reclaimer for FileReclaimer {
    fn quarantine(&self, target: &ReclaimTarget, stamp: u64) -> Result<Quarantined, ReclaimError> {
        let (canonical_target, canonical_root) = self.verify(target)?;
        let size = path_size(&canonical_target);
        if size != target.size {
            // The plan said one thing and the disk says another: refuse rather than move
            // something that changed after it was previewed.
            return Err(ReclaimError::Unsafe {
                path: target.path.clone(),
                reason: format!(
                    "its size changed since the preview ({} bytes then, {size} bytes now)",
                    target.size
                ),
            });
        }

        let batch_directory = self.quarantine_root.join(stamp.to_string());
        std::fs::create_dir_all(&batch_directory).map_err(|error| ReclaimError::Move {
            path: target.path.clone(),
            message: error.to_string(),
        })?;

        let name = canonical_target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "entry".to_string());
        let mut location = batch_directory.join(&name);
        let mut suffix = 1;
        while location.exists() {
            location = batch_directory.join(format!("{name}.{suffix}"));
            suffix += 1;
        }

        // A rename across filesystems fails; that failure is reported, never translated
        // into a copy-and-delete.
        std::fs::rename(&canonical_target, &location).map_err(|error| ReclaimError::Move {
            path: target.path.clone(),
            message: error.to_string(),
        })?;

        let entry = Quarantined {
            original: canonical_target,
            location,
            root: canonical_root,
            size,
            stamp,
            moved_at_unix: now_unix(),
        };
        if let Err(error) = self.append_ledger(&entry) {
            // Without a ledger line a restore is impossible, so the move is undone.
            let _ = std::fs::rename(&entry.location, &entry.original);
            return Err(error);
        }
        Ok(entry)
    }

    fn entries(&self) -> Result<Vec<Quarantined>, ReclaimError> {
        self.read_ledger()
    }

    fn restore(&self, entry: &Quarantined) -> Result<(), ReclaimError> {
        let known = self
            .read_ledger()?
            .into_iter()
            .any(|known| known.location == entry.location && known.original == entry.original);
        if !known {
            return Err(ReclaimError::Restore {
                path: entry.location.clone(),
                message: "this entry is not in the quarantine ledger".to_string(),
            });
        }
        if !entry.location.exists() {
            return Err(ReclaimError::Restore {
                path: entry.location.clone(),
                message: "the quarantined item is gone".to_string(),
            });
        }
        if entry.original.exists() {
            return Err(ReclaimError::Restore {
                path: entry.original.clone(),
                message: "something already occupies the original path".to_string(),
            });
        }
        if let Some(parent) = entry.original.parent() {
            std::fs::create_dir_all(parent).map_err(|error| ReclaimError::Restore {
                path: entry.original.clone(),
                message: error.to_string(),
            })?;
        }
        std::fs::rename(&entry.location, &entry.original).map_err(|error| ReclaimError::Restore {
            path: entry.original.clone(),
            message: error.to_string(),
        })
    }

    fn purge(&self, entry: &Quarantined) -> Result<(), ReclaimError> {
        // Only something this ledger recorded can be purged: an arbitrary path can never
        // reach a permanent delete through this door.
        let known = self
            .read_ledger()?
            .into_iter()
            .find(|known| known.location == entry.location);
        let Some(known) = known else {
            return Err(ReclaimError::Purge {
                path: entry.location.clone(),
                message: "this entry is not in the quarantine ledger".to_string(),
            });
        };
        if !known.location.starts_with(&self.quarantine_root) {
            return Err(ReclaimError::Purge {
                path: known.location.clone(),
                message: "the recorded location is outside the quarantine area".to_string(),
            });
        }

        let metadata =
            std::fs::symlink_metadata(&known.location).map_err(|_| ReclaimError::Purge {
                path: known.location.clone(),
                message: "the quarantined item is already gone".to_string(),
            })?;
        if metadata.file_type().is_symlink() {
            // Remove the link itself, never what it points at.
            return std::fs::remove_file(&known.location).map_err(|error| ReclaimError::Purge {
                path: known.location.clone(),
                message: error.to_string(),
            });
        }
        if metadata.is_dir() {
            std::fs::remove_dir_all(&known.location).map_err(|error| ReclaimError::Purge {
                path: known.location.clone(),
                message: error.to_string(),
            })
        } else {
            std::fs::remove_file(&known.location).map_err(|error| ReclaimError::Purge {
                path: known.location.clone(),
                message: error.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sandbox {
        dir: PathBuf,
    }

    impl Sandbox {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("occ-reclaim-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("root/.plugin.bak.123")).expect("leftover");
            std::fs::write(dir.join("root/.plugin.bak.123/manifest.json"), b"{}").expect("file");
            std::fs::write(dir.join("root/keep.txt"), b"keep").expect("file");
            std::fs::create_dir_all(dir.join("state")).expect("state");
            Self { dir }
        }

        fn reclaimer(&self) -> FileReclaimer {
            FileReclaimer::new(self.dir.join("state"))
        }

        fn root(&self) -> PathBuf {
            self.dir.join("root")
        }

        fn target(&self) -> ReclaimTarget {
            let path = self.root().join(".plugin.bak.123");
            let size = path_size(&path);
            ReclaimTarget::quarantine(path, self.root(), size)
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn moves_a_leftover_into_quarantine_and_records_where_it_came_from() {
        let sandbox = Sandbox::new("move");
        let reclaimer = sandbox.reclaimer();
        let target = sandbox.target();

        let entry = reclaimer.quarantine(&target, 7).expect("quarantine");
        assert!(!target.path.exists(), "the leftover is gone from its root");
        assert!(entry.location.exists(), "it lives in the quarantine area");
        assert_eq!(
            entry.original,
            target.path.canonicalize().unwrap_or(target.path.clone())
        );
        assert_eq!(entry.stamp, 7);
        assert!(reclaimer.quarantine_root().join("7").exists());

        let entries = reclaimer.entries().expect("entries");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].location, entry.location);
    }

    #[test]
    fn a_restore_puts_it_back_exactly_where_it_was() {
        let sandbox = Sandbox::new("restore");
        let reclaimer = sandbox.reclaimer();
        let target = sandbox.target();
        let entry = reclaimer.quarantine(&target, 1).expect("quarantine");

        reclaimer.restore(&entry).expect("restore");
        assert!(target.path.exists(), "the leftover is back");
        assert!(!entry.location.exists());
        assert_eq!(
            std::fs::read_to_string(target.path.join("manifest.json")).expect("file"),
            "{}"
        );
    }

    #[test]
    fn a_purge_only_accepts_an_entry_the_ledger_knows() {
        let sandbox = Sandbox::new("purge");
        let reclaimer = sandbox.reclaimer();
        let target = sandbox.target();
        let entry = reclaimer.quarantine(&target, 2).expect("quarantine");

        let invented = Quarantined {
            location: sandbox.root().join("keep.txt"),
            ..entry.clone()
        };
        assert!(
            reclaimer.purge(&invented).is_err(),
            "an arbitrary path must never be purged"
        );

        let size = path_size(&sandbox.root().join("keep.txt"));
        let plant = Quarantined {
            location: sandbox.root().join("keep.txt"),
            original: sandbox.root().join("somewhere"),
            root: sandbox.root(),
            size,
            stamp: 2,
            moved_at_unix: 0,
        };
        assert!(reclaimer.purge(&plant).is_err());

        reclaimer.purge(&entry).expect("purge the recorded entry");
        assert!(!entry.location.exists());
        assert!(
            sandbox.root().join("keep.txt").exists(),
            "unrelated files are untouched"
        );
    }

    #[test]
    fn a_target_that_is_a_symlink_is_refused() {
        let sandbox = Sandbox::new("symlink");
        let reclaimer = sandbox.reclaimer();
        let link = sandbox.root().join("link");
        std::os::unix::fs::symlink(sandbox.root().join("keep.txt"), &link).expect("symlink");

        let size = path_size(&link);
        let target = ReclaimTarget::quarantine(link.clone(), sandbox.root(), size);
        let error = reclaimer.quarantine(&target, 3).expect_err("must refuse");
        assert_eq!(error, ReclaimError::Symlink { path: link });
        assert!(sandbox.root().join("keep.txt").exists());
    }

    #[test]
    fn a_target_outside_its_declared_root_is_refused() {
        let sandbox = Sandbox::new("outside");
        let reclaimer = sandbox.reclaimer();
        let nested = sandbox.root().join(".plugin.bak.123/manifest.json");
        let size = path_size(&nested);
        let target = ReclaimTarget::quarantine(nested.clone(), sandbox.root(), size);

        let error = reclaimer.quarantine(&target, 4).expect_err("must refuse");
        assert!(
            matches!(error, ReclaimError::OutsideRoot { .. }),
            "unexpected: {error}"
        );
        assert!(nested.exists());
    }

    #[test]
    fn a_target_that_changed_size_since_the_preview_is_refused() {
        let sandbox = Sandbox::new("drift");
        let reclaimer = sandbox.reclaimer();
        let path = sandbox.root().join(".plugin.bak.123");
        let target = ReclaimTarget::quarantine(path.clone(), sandbox.root(), 0);

        let error = reclaimer.quarantine(&target, 5).expect_err("must refuse");
        assert!(
            matches!(error, ReclaimError::Unsafe { .. }),
            "unexpected: {error}"
        );
        assert!(path.exists(), "nothing moved");
    }

    #[test]
    fn the_root_itself_can_never_be_reclaimed() {
        let sandbox = Sandbox::new("root");
        let reclaimer = sandbox.reclaimer();
        let root = sandbox.root();
        let target = ReclaimTarget::quarantine(root.clone(), root.clone(), path_size(&root));
        let error = reclaimer.quarantine(&target, 6).expect_err("must refuse");
        assert!(
            matches!(error, ReclaimError::Unsafe { .. }),
            "unexpected: {error}"
        );
    }

    #[test]
    fn two_entries_with_the_same_name_do_not_overwrite_each_other() {
        let sandbox = Sandbox::new("collision");
        let reclaimer = sandbox.reclaimer();
        let first = sandbox.target();
        let first_entry = reclaimer.quarantine(&first, 8).expect("first");

        std::fs::create_dir_all(sandbox.root().join(".plugin.bak.123")).expect("again");
        std::fs::write(sandbox.root().join(".plugin.bak.123/other.json"), b"{}").expect("file");
        let second = ReclaimTarget::quarantine(
            sandbox.root().join(".plugin.bak.123"),
            sandbox.root(),
            path_size(&sandbox.root().join(".plugin.bak.123")),
        );
        let second_entry = reclaimer.quarantine(&second, 8).expect("second");

        assert_ne!(first_entry.location, second_entry.location);
        assert!(first_entry.location.exists());
        assert!(second_entry.location.exists());
    }

    #[test]
    fn an_empty_quarantine_is_an_empty_list() {
        let sandbox = Sandbox::new("empty");
        assert!(sandbox.reclaimer().entries().expect("entries").is_empty());
    }
}
