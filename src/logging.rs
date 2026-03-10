//! Logging setup: console (stderr), file, and in-app buffer for the Log window.

use std::sync::{Arc, Mutex};
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

/// In-memory buffer for the Log window (last N lines).
const LOG_BUFFER_MAX: usize = 2000;

static LOG_BUFFER: std::sync::OnceLock<Arc<Mutex<Vec<String>>>> = std::sync::OnceLock::new();

/// Returns the shared log buffer for the UI.
pub fn log_buffer() -> Arc<Mutex<Vec<String>>> {
    LOG_BUFFER
        .get_or_init(|| Arc::new(Mutex::new(Vec::with_capacity(LOG_BUFFER_MAX))))
        .clone()
}

/// Returns the log file path (for display in UI).
pub fn log_file_path() -> Option<std::path::PathBuf> {
    dirs::data_local_dir()
        .or_else(dirs::config_dir)
        .map(|p| p.join("can-viz").join("dermo.log"))
}

/// Initialize logging: stderr (console), file, and in-memory buffer.
/// Call once at startup.
pub fn init() {
    let buffer = log_buffer();
    let log_path = log_file_path();

    // Ensure log directory exists
    if let Some(ref path) = log_path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    let file_layer = log_path.and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .ok()
            .map(|file| {
                fmt::layer()
                    .with_ansi(false)
                    .with_writer(std::sync::Mutex::new(file))
                    .with_target(false)
                    .with_thread_ids(false)
            })
    });

    let buffer_clone = buffer.clone();
    let buffer_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(false)
        .with_writer(move || BufferWriter::new(buffer_clone.clone()));

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr).with_span_events(FmtSpan::NONE))
        .with(buffer_layer);

    if let Some(fl) = file_layer {
        registry.with(fl).init();
    } else {
        registry.init();
    }
}

/// Writer that appends formatted lines to the log buffer.
struct BufferWriter {
    buffer: Arc<Mutex<Vec<String>>>,
    line: String,
}

impl BufferWriter {
    fn new(buffer: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            buffer,
            line: String::new(),
        }
    }
}

impl std::io::Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let s = String::from_utf8_lossy(buf);
        for ch in s.chars() {
            if ch == '\n' {
                let line = std::mem::take(&mut self.line);
                if !line.is_empty() {
                    if let Ok(mut v) = self.buffer.lock() {
                        v.push(line);
                        if v.len() > LOG_BUFFER_MAX {
                            let excess = v.len() - LOG_BUFFER_MAX;
                            v.drain(0..excess);
                        }
                    }
                }
            } else {
                self.line.push(ch);
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.line.is_empty() {
            if let Ok(mut v) = self.buffer.lock() {
                v.push(std::mem::take(&mut self.line));
                if v.len() > LOG_BUFFER_MAX {
                    let excess = v.len() - LOG_BUFFER_MAX;
                    v.drain(0..excess);
                }
            }
        }
        Ok(())
    }
}
