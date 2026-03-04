pub mod cabana;
pub mod csv;
pub mod rlog;

pub use cabana::{load_cabana_rlog, load_cabana_rlog_with_progress, load_cabana_session};
pub use csv::{load_csv, load_csv_with_progress, load_csv_streaming, ProgressCallback, ChunkCallback};
pub use rlog::load_rlog;

use anyhow::Result;
use crate::core::CanMessage;

/// Input format detection result
#[derive(Debug, Clone)]
pub enum InputFormat {
    Csv,
    Rlog,
    CabanaRlog,
    Unknown,
}

/// Detect the format of an input file by checking the file header/magic
pub fn detect_format(data: &[u8]) -> InputFormat {
    // bzip2 compressed rlog (openpilot standard)
    if is_rlog_bz2(data) {
        return InputFormat::Rlog;
    }

    // Cabana/uncompressed rlog: Cap'n Proto stream (segment table)
    if is_cabana_rlog(data) {
        return InputFormat::CabanaRlog;
    }

    // Check if it looks like CSV (text, comma separated)
    if is_csv(data) {
        return InputFormat::Csv;
    }

    InputFormat::Unknown
}

fn is_rlog_bz2(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == b'B' && data[1] == b'Z'
}

/// Cabana rlog: uncompressed Cap'n Proto. First 4 bytes = (segment_count-1), typically 0 for 1 segment.
fn is_cabana_rlog(data: &[u8]) -> bool {
    if data.len() < 8 {
        return false;
    }
    let seg_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize + 1;
    // Sanity: 1-64 segments, next 4 bytes = segment size in words (reasonable)
    seg_count >= 1 && seg_count <= 64
}

fn is_csv(data: &[u8]) -> bool {
    // Check if the data looks like CSV (text with commas)
    // Look for a line with commas in the first 500 bytes
    if data.len() < 10 {
        return false;
    }

    let sample = std::str::from_utf8(&data[..data.len().min(500)]);
    match sample {
        Ok(text) => {
            // Check for CSV-like patterns (multiple commas on a line)
            text.lines().take(5).any(|line| line.chars().filter(|&c| c == ',').count() >= 2)
        }
        Err(_) => false,
    }
}

/// Load CAN data from a file, auto-detecting format
pub fn load_file(path: &str) -> Result<Vec<CanMessage>> {
    load_file_with_progress(path, None)
}

/// Load CAN data with optional progress callback. For CSV, calls progress_cb(current_bytes, total_bytes).
pub fn load_file_with_progress(
    path: &str,
    progress_cb: Option<ProgressCallback>,
) -> Result<Vec<CanMessage>> {
    // Only read first 1KB for format detection to avoid loading large files twice
    let mut f = std::fs::File::open(path)?;
    let mut header = vec![0u8; 1024];
    let n = std::io::Read::read(&mut f, &mut header)?;
    header.truncate(n);

    match detect_format(&header) {
        InputFormat::Csv => load_csv_with_progress(path, progress_cb),
        InputFormat::Rlog | InputFormat::CabanaRlog => {
            load_cabana_rlog_with_progress(path, progress_cb)
        }
        InputFormat::Unknown => anyhow::bail!("Unknown input format"),
    }
}

/// Stream load CSV: calls chunk_cb with each batch, progress_cb for progress. Returns Ok(()) when done.
pub fn load_file_streaming(
    path: &str,
    chunk_cb: ChunkCallback,
    progress_cb: Option<ProgressCallback>,
) -> Result<()> {
    let mut f = std::fs::File::open(path)?;
    let mut header = vec![0u8; 1024];
    let n = std::io::Read::read(&mut f, &mut header)?;
    header.truncate(n);

    match detect_format(&header) {
        InputFormat::Csv => load_csv_streaming(path, chunk_cb, progress_cb),
        InputFormat::Rlog | InputFormat::CabanaRlog => {
            // rlog/cabana don't support streaming - fall back to full load
            let messages = load_cabana_rlog(path)?;
            chunk_cb(messages);
            Ok(())
        }
        InputFormat::Unknown => anyhow::bail!("Unknown input format"),
    }
}
