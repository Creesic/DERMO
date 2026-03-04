use anyhow::{Context, Result};
use std::path::Path;
use crate::core::CanMessage;
use chrono::Utc;

/// Column layout for CSV parsing
#[derive(Debug)]
enum CsvLayout {
    /// Single data column (time,bus,id,data)
    SingleData { time_idx: usize, bus_idx: usize, id_idx: usize, data_idx: usize },
    /// driveSAV format: Time Stamp, ID, Bus, LEN, D1..D8
    DriveSav { time_idx: usize, bus_idx: usize, id_idx: usize, len_idx: usize, d_indices: [usize; 8] },
}

/// Callback for progress during streaming load: (current_byte_offset, total_bytes)
pub type ProgressCallback = Box<dyn Fn(usize, usize) + Send>;

/// Callback for streaming chunk: receives batch of messages
pub type ChunkCallback = Box<dyn Fn(Vec<CanMessage>) + Send>;

/// Load CSV in chunks, calling chunk_cb with each batch. Also calls progress_cb for progress.
/// Chunk size is ~5000 messages.
pub fn load_csv_streaming(
    path: &str,
    chunk_cb: ChunkCallback,
    progress_cb: Option<ProgressCallback>,
) -> Result<()> {
    const CHUNK_SIZE: usize = 5000;

    let file_path = Path::new(path);
    let total_bytes = std::fs::metadata(file_path).map(|m| m.len() as usize).unwrap_or(0);

    if let Some(ref cb) = progress_cb {
        cb(0, total_bytes.max(1));
    }

    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(file_path)?;

    let headers = rdr.headers()?;
    let layout = detect_columns(headers)?;

    let mut batch = Vec::with_capacity(CHUNK_SIZE);
    let mut accumulated_time_secs = 0.0;
    let mut last_seen_time = 0.0;
    let time_is_microseconds = matches!(&layout, CsvLayout::DriveSav { .. });
    let base_time = Utc::now();
    let mut record_count = 0usize;

    for result in rdr.records() {
        let record = result.context("Failed to read CSV row")?;
        record_count += 1;

        if let Some(ref cb) = progress_cb {
            let estimated_bytes = (record_count * 50).min(total_bytes);
            if record_count % 5000 == 0 || estimated_bytes >= total_bytes {
                cb(estimated_bytes.min(total_bytes), total_bytes.max(1));
            }
        }

        let (time_relative, bus, id, data) = match &layout {
            CsvLayout::SingleData { time_idx, bus_idx, id_idx, data_idx } => {
                let time_val = record.get(*time_idx).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                let bus = record.get(*bus_idx).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                let id = parse_can_id(record.get(*id_idx).context("Missing ID column")?)?;
                let hex_data = record.get(*data_idx).context("Missing data column")?;
                let data = CanMessage::parse_hex(hex_data)?;
                (time_val, bus, id, data)
            }
            CsvLayout::DriveSav { time_idx, bus_idx, id_idx, len_idx, d_indices } => {
                let time_val = record.get(*time_idx).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                let bus = record.get(*bus_idx).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                let id = parse_can_id(record.get(*id_idx).context("Missing ID column")?)?;
                let len: usize = record.get(*len_idx).and_then(|s| s.parse().ok()).unwrap_or(8).min(8);
                let mut data = Vec::with_capacity(len);
                for i in 0..len {
                    if let Some(&di) = d_indices.get(i) {
                        if let Some(hex_byte) = record.get(di) {
                            if let Ok(b) = u8::from_str_radix(hex_byte.trim(), 16) {
                                data.push(b);
                            }
                        }
                    }
                }
                (time_val, bus, id, data)
            }
        };

        let time_relative_secs = if time_is_microseconds {
            time_relative / 1_000_000.0
        } else {
            time_relative
        };

        if time_relative_secs < last_seen_time - 0.1 {
            accumulated_time_secs += 0.000001;
        } else if time_relative_secs > last_seen_time {
            accumulated_time_secs += time_relative_secs - last_seen_time;
        }
        last_seen_time = time_relative_secs;

        let us = (accumulated_time_secs * 1_000_000.0) as i64;
        let timestamp = base_time + chrono::Duration::microseconds(us);

        batch.push(CanMessage { timestamp, bus, id, data });

        if batch.len() >= CHUNK_SIZE {
            chunk_cb(std::mem::take(&mut batch));
            batch.reserve(CHUNK_SIZE);
        }
    }

    if !batch.is_empty() {
        chunk_cb(batch);
    }

    Ok(())
}

/// Load CAN messages from a CSV file with progress callback.
/// Calls progress_cb(current_byte, total_bytes) during parsing.
pub fn load_csv_with_progress(
    path: &str,
    progress_cb: Option<ProgressCallback>,
) -> Result<Vec<CanMessage>> {
    let file_path = Path::new(path);
    let total_bytes = std::fs::metadata(file_path).map(|m| m.len() as usize).unwrap_or(0);

    // Immediate progress so UI shows something right away
    if let Some(ref cb) = progress_cb {
        cb(0, total_bytes.max(1));
    }

    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(file_path)?;

    let headers = rdr.headers()?;
    let layout = detect_columns(headers)?;

    let mut messages = Vec::new();
    let mut accumulated_time_secs = 0.0;
    let mut last_seen_time = 0.0;
    let time_is_microseconds = matches!(&layout, CsvLayout::DriveSav { .. });
    let base_time = Utc::now();
    let mut record_count = 0usize;

    for result in rdr.records() {
        let record = result.context("Failed to read CSV row")?;
        record_count += 1;

        // Progress: estimate bytes from record count (avg ~50 bytes/record for CSV)
        if let Some(ref cb) = progress_cb {
            let estimated_bytes = (record_count * 50).min(total_bytes);
            if record_count % 5000 == 0 || estimated_bytes >= total_bytes {
                cb(estimated_bytes.min(total_bytes), total_bytes.max(1));
            }
        }

        let (time_relative, bus, id, data) = match &layout {
            CsvLayout::SingleData { time_idx, bus_idx, id_idx, data_idx } => {
                let time_val = record.get(*time_idx).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                let bus = record.get(*bus_idx).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                let id = parse_can_id(record.get(*id_idx).context("Missing ID column")?)?;
                let hex_data = record.get(*data_idx).context("Missing data column")?;
                let data = CanMessage::parse_hex(hex_data)?;
                (time_val, bus, id, data)
            }
            CsvLayout::DriveSav { time_idx, bus_idx, id_idx, len_idx, d_indices } => {
                let time_val = record.get(*time_idx).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                let bus = record.get(*bus_idx).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                let id = parse_can_id(record.get(*id_idx).context("Missing ID column")?)?;
                let len: usize = record.get(*len_idx).and_then(|s| s.parse().ok()).unwrap_or(8).min(8);
                let mut data = Vec::with_capacity(len);
                for i in 0..len {
                    if let Some(&di) = d_indices.get(i) {
                        if let Some(hex_byte) = record.get(di) {
                            if let Ok(b) = u8::from_str_radix(hex_byte.trim(), 16) {
                                data.push(b);
                            }
                        }
                    }
                }
                (time_val, bus, id, data)
            }
        };

        let time_relative_secs = if time_is_microseconds {
            time_relative / 1_000_000.0
        } else {
            time_relative
        };

        if time_relative_secs < last_seen_time - 0.1 {
            accumulated_time_secs += 0.000001;
        } else if time_relative_secs > last_seen_time {
            accumulated_time_secs += time_relative_secs - last_seen_time;
        }
        last_seen_time = time_relative_secs;

        let us = (accumulated_time_secs * 1_000_000.0) as i64;
        let timestamp = base_time + chrono::Duration::microseconds(us);

        messages.push(CanMessage { timestamp, bus, id, data });
    }

    Ok(messages)
}

/// Load CAN messages from a CSV file
///
/// Supports flexible column formats:
/// - time,bus,msg_id,data
/// - timestamp,can_id,payload
/// - time,id,hex_data
/// - driveSAV: Time Stamp,ID,Extended,Dir,Bus,LEN,D1,D2,D3,D4,D5,D6,D7,D8
///
/// Timestamps are treated as relative seconds (or microseconds for driveSAV) from the start of the log
pub fn load_csv(path: &str) -> Result<Vec<CanMessage>> {
    let file_path = Path::new(path);
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(file_path)?;

    let headers = rdr.headers()?;
    let layout = detect_columns(headers)?;

    let mut messages = Vec::new();

    // Use the first message's actual time as base, and accumulate for subsequent messages
    let mut accumulated_time_secs = 0.0;
    let mut last_seen_time = 0.0;
    let time_is_microseconds = matches!(&layout, CsvLayout::DriveSav { .. });

    // Get base time as NOW for absolute timestamps
    let base_time = Utc::now();

    for result in rdr.records() {
        let record = result.context("Failed to read CSV row")?;

        let (time_relative, bus, id, data) = match &layout {
            CsvLayout::SingleData { time_idx, bus_idx, id_idx, data_idx } => {
                let time_val = record.get(*time_idx).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                let bus = record.get(*bus_idx).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                let id = parse_can_id(record.get(*id_idx).context("Missing ID column")?)?;
                let hex_data = record.get(*data_idx).context("Missing data column")?;
                let data = CanMessage::parse_hex(hex_data)?;
                (time_val, bus, id, data)
            }
            CsvLayout::DriveSav { time_idx, bus_idx, id_idx, len_idx, d_indices } => {
                let time_val = record.get(*time_idx).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                let bus = record.get(*bus_idx).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                let id = parse_can_id(record.get(*id_idx).context("Missing ID column")?)?;
                let len: usize = record.get(*len_idx).and_then(|s| s.parse().ok()).unwrap_or(8).min(8);
                let mut data = Vec::with_capacity(len);
                for i in 0..len {
                    if let Some(&di) = d_indices.get(i) {
                        if let Some(hex_byte) = record.get(di) {
                            if let Ok(b) = u8::from_str_radix(hex_byte.trim(), 16) {
                                data.push(b);
                            }
                        }
                    }
                }
                (time_val, bus, id, data)
            }
        };

        // Normalize time to seconds (driveSAV uses microseconds)
        let time_relative_secs = if time_is_microseconds {
            time_relative / 1_000_000.0
        } else {
            time_relative
        };

        // Track accumulated time - handle both forward time and resets
        if time_relative_secs < last_seen_time - 0.1 {
            // Time jumped back significantly - this is likely a new session
            accumulated_time_secs += 0.000001;  // Small increment for the new message
        } else if time_relative_secs > last_seen_time {
            // Time moved forward - add the difference
            accumulated_time_secs += time_relative_secs - last_seen_time;
        }
        last_seen_time = time_relative_secs;

        // Calculate actual timestamp (microsecond precision)
        let us = (accumulated_time_secs * 1_000_000.0) as i64;
        let timestamp = base_time + chrono::Duration::microseconds(us);

        messages.push(CanMessage { timestamp, bus, id, data });
    }

    Ok(messages)
}

/// Parse CAN ID - supports decimal, 0x-prefixed hex, and bare hex (e.g. 00000197)
fn parse_can_id(s: &str) -> Result<u32> {
    let s = s.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        return u32::from_str_radix(&s[2..], 16).map_err(|e| anyhow::anyhow!("Failed to parse CAN ID: {}", e));
    }
    // Try hex first if it looks like hex (6-8 hex digits, e.g. driveSAV format)
    if s.len() >= 4 && s.len() <= 8 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        if let Ok(id) = u32::from_str_radix(s, 16) {
            return Ok(id);
        }
    }
    s.parse::<u32>().map_err(|e| anyhow::anyhow!("Failed to parse CAN ID: {}", e))
}

/// Detect column layout from CSV headers
fn detect_columns(headers: &csv::StringRecord) -> Result<CsvLayout> {
    // Check for driveSAV format: Time Stamp, ID, Bus, LEN, D1..D8
    if let Ok(drivesav) = detect_drivesav_layout(headers) {
        return Ok(drivesav);
    }

    // Standard single-data-column format
    let time_idx = find_column(headers, &["time", "timestamp", "t", "ts", "time stamp", "time_stamp"])?;
    let bus_idx = find_column(headers, &["bus", "channel", "interface"])?;
    let id_idx = find_column(headers, &["id", "addr", "msg_id", "can_id", "message_id"])?;
    let data_idx = find_column(headers, &["data", "payload", "hex", "bytes"])?;

    Ok(CsvLayout::SingleData { time_idx, bus_idx, id_idx, data_idx })
}

/// Detect driveSAV layout: Time Stamp,ID,Extended,Dir,Bus,LEN,D1,D2,...,D8
fn detect_drivesav_layout(headers: &csv::StringRecord) -> Result<CsvLayout> {
    let time_idx = find_column(headers, &["time stamp", "time_stamp", "timestamp"])?;
    let bus_idx = find_column(headers, &["bus"])?;
    let id_idx = find_column(headers, &["id"])?;
    let len_idx = find_column(headers, &["len", "length"])?;

    let mut d_indices = [0usize; 8];
    for i in 0..8 {
        let d_name = format!("d{}", i + 1);
        d_indices[i] = find_column(headers, &[d_name.as_str()])?;
    }

    Ok(CsvLayout::DriveSav {
        time_idx,
        bus_idx,
        id_idx,
        len_idx,
        d_indices,
    })
}

/// Find a column by checking possible names
fn find_column(headers: &csv::StringRecord, names: &[&str]) -> Result<usize> {
    for (idx, header) in headers.iter().enumerate() {
        let header_lower = header.to_lowercase();
        if names.iter().any(|&name| header_lower == name) {
            return Ok(idx);
        }
    }

    anyhow::bail!("Could not find column with names: {:?}", names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_parse_hex() {
        assert_eq!(
            CanMessage::parse_hex("12 34 AB CD").unwrap(),
            vec![0x12, 0x34, 0xAB, 0xCD]
        );
        assert_eq!(
            CanMessage::parse_hex("1234ABCD").unwrap(),
            vec![0x12, 0x34, 0xAB, 0xCD]
        );
    }

    #[test]
    fn test_parse_can_id() {
        assert_eq!(parse_can_id("0x197").unwrap(), 0x197);
        assert_eq!(parse_can_id("00000197").unwrap(), 0x197);
        assert_eq!(parse_can_id("407").unwrap(), 407);
    }

    #[test]
    fn test_load_drivesav_format() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_drivesav.csv");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "Time Stamp,ID,Extended,Dir,Bus,LEN,D1,D2,D3,D4,D5,D6,D7,D8").unwrap();
        // Match real driveSAV format (including trailing comma)
        writeln!(f, "0,00000197,false,Rx,0,4,83,0C,0E,C0,00,00,00,00,").unwrap();
        writeln!(f, "8000,000000D9,false,Rx,0,8,80,1B,00,10,00,F0,7F,C0,").unwrap();
        drop(f);

        let msgs = load_csv(path.to_str().unwrap()).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].id, 0x197);
        assert_eq!(msgs[0].data, vec![0x83, 0x0C, 0x0E, 0xC0]);
        assert_eq!(msgs[1].id, 0xD9);
        assert_eq!(msgs[1].data, vec![0x80, 0x1B, 0x00, 0x10, 0x00, 0xF0, 0x7F, 0xC0]);

        let _ = std::fs::remove_file(&path);
    }
}
