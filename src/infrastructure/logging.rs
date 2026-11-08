//! Diagnostic logging to a file. The TUI owns the terminal, so nothing is ever
//! logged to stderr while the interface is running.
//!
//! The log records commands, exit codes and error summaries. It never records
//! environment variables, credentials or configuration file contents.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone)]
struct FileLogger {
    file: Arc<Mutex<File>>,
}

struct FileWriter {
    file: Arc<Mutex<File>>,
}

impl Write for FileWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        // A logging failure must never take the application down: a poisoned lock
        // or a full disk simply drops the line.
        match self.file.lock() {
            Ok(mut file) => file
                .write_all(buffer)
                .map(|()| buffer.len())
                .or(Ok(buffer.len())),
            Err(_) => Ok(buffer.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.file.lock() {
            Ok(mut file) => file.flush().or(Ok(())),
            Err(_) => Ok(()),
        }
    }
}

impl<'a> MakeWriter<'a> for FileLogger {
    type Writer = FileWriter;

    fn make_writer(&'a self) -> Self::Writer {
        FileWriter {
            file: Arc::clone(&self.file),
        }
    }
}

/// Installs the log subscriber. Repeated calls are ignored so tests can share a
/// process without panicking.
pub fn init(log_path: &Path, level: Option<&str>) -> io::Result<()> {
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log_path)?;
    let logger = FileLogger {
        file: Arc::new(Mutex::new(file)),
    };
    let filter =
        EnvFilter::try_new(level.unwrap_or("info")).unwrap_or_else(|_| EnvFilter::new("info"));

    let subscriber = tracing_subscriber::fmt()
        .with_writer(logger)
        .with_ansi(false)
        .with_target(true)
        .with_level(true)
        .with_env_filter(filter)
        .finish();

    let _ = tracing::subscriber::set_global_default(subscriber);
    Ok(())
}
