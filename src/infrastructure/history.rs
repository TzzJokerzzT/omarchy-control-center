//! Operation history.
//!
//! One JSON object per line, owner-only permissions, append-only. The record is
//! built from the plan and the real outcome and contains no credentials and no
//! configuration content: timestamp, adapter, component, operation, result, exit
//! code, command and an error summary.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::application::OperationRecord;

#[derive(Debug, Clone)]
pub struct History {
    path: PathBuf,
}

impl History {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one record. The file is created owner-only; an existing file with
    /// looser permissions is tightened instead of trusted.
    pub fn append(&self, record: &OperationRecord) -> std::io::Result<()> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;

        let line = serde_json::to_string(record).unwrap_or_else(|_| {
            // A record that cannot be serialized must still leave evidence.
            format!("{{\"error\":\"unserialisable record for {}\"}}", record.component)
        });
        writeln!(file, "{line}")?;
        file.flush()
    }

    /// Most recent records first, at most `limit`. Unreadable or malformed lines are
    /// skipped, never guessed at.
    pub fn recent(&self, limit: usize) -> std::io::Result<Vec<OperationRecord>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = std::fs::File::open(&self.path)?;
        let mut records: Vec<OperationRecord> = Vec::new();
        for line in std::io::BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<OperationRecord>(&line) {
                Ok(record) => records.push(record),
                Err(error) => tracing::warn!(%error, "skipping an unreadable history line"),
            }
        }
        records.reverse();
        records.truncate(limit);
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("occ-history-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("history.jsonl")
    }

    fn record(component: &str, success: bool) -> OperationRecord {
        OperationRecord {
            started_at_unix: 1_700_000_000,
            finished_at_unix: 1_700_000_010,
            adapter: "pacman".to_string(),
            component: component.to_string(),
            component_name: component.to_string(),
            kind: "remove".to_string(),
            success,
            exit_code: Some(if success { 0 } else { 1 }),
            command: format!("pacman -R {component}"),
            summary: "done".to_string(),
            error_tail: String::new(),
            confirmed_by: format!("typed identifier {component}"),
        }
    }

    #[test]
    fn appends_and_reads_back_most_recent_first() {
        let path = temp_path("order");
        let history = History::new(path.clone());
        history.append(&record("btop", true)).expect("append");
        history.append(&record("htop", false)).expect("append");

        let recent = history.recent(10).expect("read");
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].component, "htop", "newest first");
        assert_eq!(recent[1].component, "btop");
        assert!(recent[1].success);
        assert!(!recent[0].success);
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn the_history_file_is_owner_only() {
        let path = temp_path("mode");
        let history = History::new(path.clone());
        history.append(&record("btop", true)).expect("append");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn a_malformed_line_is_skipped_without_losing_the_rest() {
        let path = temp_path("malformed");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("dir");
        }
        std::fs::write(&path, "not json\n{}\n").expect("seed");
        let history = History::new(path.clone());
        history.append(&record("btop", true)).expect("append");
        let recent = history.recent(10).expect("read");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].component, "btop");
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn a_missing_file_is_an_empty_history() {
        let path = temp_path("absent");
        assert!(History::new(path).recent(5).expect("read").is_empty());
    }

    #[test]
    fn limits_the_number_of_records_returned() {
        let path = temp_path("limit");
        let history = History::new(path.clone());
        for name in ["a", "b", "c"] {
            history.append(&record(name, true)).expect("append");
        }
        let recent = history.recent(2).expect("read");
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].component, "c");
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }
}
