//! Cabana rlog format parser.
//!
//! Cabana (openpilot's CAN visualizer) records live streams to rlog files.
//! Each recording session creates a folder (e.g. 2025-06-25--21-24-49--0) with
//! an uncompressed rlog file containing Cap'n Proto serialized Event messages.
//! Standard openpilot rlog.bz2 is bzip2-compressed; Cabana writes uncompressed.

use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;
use crate::core::CanMessage;
use chrono::{DateTime, Duration, Utc};

/// Load CAN messages from a single Cabana/openpilot rlog file.
/// Handles both uncompressed (Cabana) and bzip2-compressed (openpilot) formats.
pub fn load_cabana_rlog(path: &str) -> Result<Vec<CanMessage>> {
    load_cabana_rlog_with_progress(path, None)
}

/// Load with optional progress callback: progress_cb(current_bytes, total_bytes)
pub fn load_cabana_rlog_with_progress(
    path: &str,
    progress_cb: Option<Box<dyn Fn(usize, usize) + Send>>,
) -> Result<Vec<CanMessage>> {
    let path_obj = Path::new(path);
    let total_bytes = std::fs::metadata(path_obj).map(|m| m.len() as usize).unwrap_or(0);

    if let Some(ref cb) = progress_cb {
        cb(0, total_bytes.max(1));
    }

    let data = std::fs::read(path_obj)
        .with_context(|| format!("Failed to read {}", path))?;

    let data = if data.starts_with(b"BZ") {
        // bzip2 compressed (standard openpilot rlog.bz2)
        let mut decompressed = Vec::new();
        bzip2::read::BzDecoder::new(&data[..])
            .read_to_end(&mut decompressed)
            .context("Failed to decompress bzip2")?;
        decompressed
    } else {
        // Uncompressed (Cabana rlog)
        data
    };

    let mut messages = Vec::new();
    let mut offset = 0usize;
    let base_time = Utc::now();
    let mut first_mono_time: Option<u64> = None;

    while offset < data.len() {
        if let Some(ref cb) = progress_cb {
            cb(offset.min(total_bytes), total_bytes.max(1));
        }

        let (msg_size, can_msgs) = match parse_one_message(&data[offset..], &base_time, &mut first_mono_time) {
            Ok((size, msgs)) => (size, msgs),
            Err(_) => break, // End of valid data or parse error
        };

        messages.extend(can_msgs);
        offset += msg_size;

        if msg_size == 0 {
            break;
        }
    }

    Ok(messages)
}

/// Load CAN messages from a Cabana session folder.
/// Loads only the segments for the selected recording (e.g. 2025-09-18--09-13-03--0 through --22).
/// Handles:
/// 1. Segment folder (e.g. 2025-09-18--09-13-03--0): finds sibling segments with same timestamp prefix only
/// 2. Session root (e.g. cabana_live_stream): recursively finds all rlog files (all recordings)
pub fn load_cabana_session(folder_path: &str) -> Result<Vec<CanMessage>> {
    let folder = Path::new(folder_path);
    if !folder.is_dir() {
        anyhow::bail!("Not a directory: {}", folder_path);
    }

    let mut rlog_paths: Vec<std::path::PathBuf> = Vec::new();

    // Case 1: Folder has rlog directly (user picked a segment folder like 2025-09-18--09-13-03--0)
    // → Look at PARENT for sibling segments with SAME session prefix only (--0 through --22 for that recording)
    let direct_rlog = folder.join("rlog");
    if direct_rlog.exists() {
        if let (Some(parent), Some(folder_name)) = (folder.parent(), folder.file_name()) {
            let session_prefix = session_prefix_from_segment_name(&folder_name.to_string_lossy());
            collect_session_rlog_files(parent, &session_prefix, &mut rlog_paths)?;
        }
        if rlog_paths.is_empty() {
            rlog_paths.push(direct_rlog);
        }
    } else {
        // Case 2: Recursively find all rlog files (user picked session root like cabana_live_stream)
        collect_rlog_files(folder, 0, 3, &mut rlog_paths)?;
    }

    if rlog_paths.is_empty() {
        anyhow::bail!("No rlog files found in {}", folder_path);
    }

    // Sort by segment number so playback is continuous (--0, --1, --2, ...)
    rlog_paths.sort_by(|a, b| {
        let na = a.parent().and_then(|p| p.file_name()).unwrap_or_default();
        let nb = b.parent().and_then(|p| p.file_name()).unwrap_or_default();
        segment_sort_key(&na.to_string_lossy()).cmp(&segment_sort_key(&nb.to_string_lossy()))
    });

    let mut all_messages = Vec::new();
    let mut timeline_end: Option<DateTime<Utc>> = None;

    for rlog_path in rlog_paths {
        if let Some(s) = rlog_path.to_str() {
            match load_cabana_rlog(s) {
                Ok(mut msgs) => {
                    if !msgs.is_empty() {
                        // Rebase timestamps so each segment continues after the previous.
                        // Hardware disconnect/reconnect resets device clock, causing overlapping
                        // timestamps across segments. Place each 1-min rlog sequentially.
                        if let Some(end) = timeline_end {
                            let seg_min = msgs.iter().map(|m| m.timestamp).min().unwrap();
                            let offset = end - seg_min + Duration::milliseconds(1);
                            for m in &mut msgs {
                                m.timestamp = m.timestamp + offset;
                            }
                        }
                        if let Some(last) = msgs.iter().map(|m| m.timestamp).max() {
                            timeline_end = Some(last);
                        }
                    }
                    all_messages.extend(msgs);
                }
                Err(e) => tracing::warn!("Failed to load {}: {}", rlog_path.display(), e),
            }
        }
    }

    Ok(all_messages)
}

/// Extract session prefix from segment folder name.
/// "2025-09-18--09-13-03--0" -> "2025-09-18--09-13-03"
fn session_prefix_from_segment_name(name: &str) -> String {
    let parts: Vec<&str> = name.rsplitn(2, "--").collect();
    parts.get(1).unwrap_or(&name).to_string()
}

/// Collect rlog paths only from sibling folders matching the session prefix.
/// e.g. prefix "2025-09-18--09-13-03" matches --0, --1, ... --22 but not other recordings
fn collect_session_rlog_files(
    parent: &Path,
    session_prefix: &str,
    out: &mut Vec<std::path::PathBuf>,
) -> Result<()> {
    let prefix_with_dash = format!("{}--", session_prefix);
    for entry in std::fs::read_dir(parent).context("Failed to read directory")?.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
            if name.starts_with(&prefix_with_dash) {
                let rlog = path.join("rlog");
                if rlog.exists() {
                    out.push(rlog);
                }
            }
        }
    }
    Ok(())
}

/// Recursively collect rlog file paths, up to max_depth levels.
fn collect_rlog_files(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<std::path::PathBuf>,
) -> Result<()> {
    if depth >= max_depth {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).context("Failed to read directory")?.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            let rlog = path.join("rlog");
            if rlog.exists() {
                out.push(rlog);
            } else {
                collect_rlog_files(&path, depth + 1, max_depth, out)?;
            }
        }
    }
    Ok(())
}

/// Parse one Cap'n Proto message from the buffer, extract CAN messages.
/// Returns (bytes_consumed, can_messages).
fn parse_one_message(
    data: &[u8],
    base_time: &DateTime<Utc>,
    first_mono_time: &mut Option<u64>,
) -> Result<(usize, Vec<CanMessage>)> {
    if data.len() < 8 {
        anyhow::bail!("Truncated");
    }

    // Cap'n Proto stream format: segment table
    // 4 bytes: (segment_count - 1) as u32 LE
    let seg_count_minus_1 = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let seg_count = seg_count_minus_1 as usize + 1;

    if seg_count == 0 || seg_count > 512 {
        anyhow::bail!("Invalid segment count");
    }

    // 4 bytes per segment: size in words (u32 LE)
    let table_size = 4 + seg_count * 4;
    if data.len() < table_size {
        anyhow::bail!("Truncated segment table");
    }

    let mut total_words = 0u64;
    for i in 0..seg_count {
        let word_offset = 4 + i * 4;
        let words = u32::from_le_bytes([
            data[word_offset],
            data[word_offset + 1],
            data[word_offset + 2],
            data[word_offset + 3],
        ]) as u64;
        total_words += words;
    }

    let message_size = table_size + (total_words as usize) * 8;
    if data.len() < message_size {
        anyhow::bail!("Truncated message");
    }

    let segment_data = &data[table_size..message_size];
    let can_msgs = extract_can_from_segment(segment_data, base_time, first_mono_time)?;

    Ok((message_size, can_msgs))
}

/// Extract CAN messages from a Cap'n Proto segment.
/// Only processes List(CanData) from the Event's can/sendcan union (pointer must be in root struct).
fn extract_can_from_segment(
    segment: &[u8],
    base_time: &DateTime<Utc>,
    first_mono_time: &mut Option<u64>,
) -> Result<Vec<CanMessage>> {
    if segment.len() < 16 {
        return Ok(vec![]);
    }

    let mut best_mono_time = 0u64;
    let mut root_ptr_section: Option<(usize, usize)> = None;

    // Root pointer at word 0 - get struct bounds so we only accept lists from Event's pointer section
    let root_ptr = read_u64_le(segment, 0);
    if (root_ptr & 3) == 0 {
        if let Ok((struct_offset, data_words, ptr_words)) = decode_struct_pointer(root_ptr) {
            if struct_offset >= 0 {
                let struct_start = ((1 + struct_offset) as usize) * 8;
                if struct_start + 8 <= segment.len() {
                    best_mono_time = read_u64_le(segment, struct_start);
                }
                // Event's pointer section: only lists stored here can be can/sendcan
                let ptr_start = 1 + struct_offset as usize + data_words as usize;
                let ptr_end = ptr_start + ptr_words as usize;
                root_ptr_section = Some((ptr_start, ptr_end));
            }
        }
    }

    let word_count = segment.len() / 8;
    let mut best_candidates: Vec<CanMessage> = Vec::new();

    // Must have valid root struct to ensure we only get Event's can/sendcan lists
    let root_ptr_section = match root_ptr_section {
        Some(r) => r,
        None => return Ok(vec![]),
    };

    for i in 0..word_count {
        let ptr = read_u64_le(segment, i * 8);
        if ptr == 0 || (ptr & 3) != 1 {
            continue;
        }
        // Only consider list pointers in the root struct's pointer section (Event's can/sendcan)
        let (start, end) = root_ptr_section;
        if i < start || i >= end {
            continue;
        }
        if let Ok(can_list) = decode_can_list(
            segment,
            i,
            ptr,
            base_time,
            first_mono_time,
            best_mono_time,
        ) {
            if !can_list.is_empty() && looks_like_can_data(&can_list) {
                // Prefer list with more unique IDs and valid 11-bit IDs
                if can_list.len() > best_candidates.len()
                    || (can_list.len() == best_candidates.len()
                        && count_unique_valid_ids(&can_list) > count_unique_valid_ids(&best_candidates))
                {
                    best_candidates = can_list;
                }
            }
        }
    }

    Ok(best_candidates)
}

/// Count unique 11-bit CAN IDs in the list (filters garbage like all-0x004)
fn count_unique_valid_ids(msgs: &[CanMessage]) -> usize {
    let mut ids = std::collections::HashSet::new();
    for m in msgs {
        if m.id <= 0x7FF {
            ids.insert(m.id);
        }
    }
    ids.len()
}

/// Heuristic: valid CAN messages - 11-bit IDs only, bus 0-3, 0-8 data bytes
fn looks_like_can_data(msgs: &[CanMessage]) -> bool {
    if msgs.is_empty() || msgs.len() > 1000 {
        return false;
    }
    let valid_count = msgs
        .iter()
        .filter(|m| {
            m.bus <= 3 && m.data.len() <= 8 && m.id <= 0x7FF // 11-bit only, no extended
        })
        .count();
    // Require majority valid and at least 2 unique IDs (reject "all 0x004" garbage)
    valid_count >= msgs.len() / 2 && count_unique_valid_ids(msgs) >= 2
}

fn decode_struct_pointer(ptr: u64) -> Result<(i32, u16, u16)> {
    if (ptr & 3) != 0 {
        anyhow::bail!("Not a struct pointer");
    }
    let offset = ((ptr >> 2) & 0x3FFF_FFFF) as i32;
    let data_words = ((ptr >> 32) & 0xFFFF) as u16;
    let ptr_words = ((ptr >> 48) & 0xFFFF) as u16;
    Ok((offset, data_words, ptr_words))
}

fn decode_can_list(
    segment: &[u8],
    ptr_word_index: usize,
    list_ptr: u64,
    base_time: &DateTime<Utc>,
    first_mono_time: &mut Option<u64>,
    event_mono_time: u64,
) -> Result<Vec<CanMessage>> {
    let offset = ((list_ptr >> 2) & 0x3FFF_FFFF) as i32;
    let elem_type = ((list_ptr >> 32) & 7) as u8;
    let _total_words = ((list_ptr >> 35) & 0x1FFF_FFFF) as usize; // For C=7: words in list, not counting tag

    if elem_type != 7 {
        return Ok(vec![]);
    }

    // List starts at (ptr_word_index + 1 + offset) words from segment start
    let list_start = (ptr_word_index + 1 + offset as usize) * 8;
    if list_start + 8 > segment.len() {
        return Ok(vec![]);
    }

    // Composite list: tag word. Tag has struct layout but B = element count, C = data words/elem, D = ptr words/elem
    let tag = read_u64_le(segment, list_start);
    let elem_count = ((tag >> 2) & 0x3FFF_FFFF) as usize;
    let struct_data_words = ((tag >> 32) & 0xFFFF) as usize;
    let struct_ptr_words = ((tag >> 48) & 0xFFFF) as usize;
    let elem_size = struct_data_words + struct_ptr_words;

    if elem_count == 0 || elem_size == 0 {
        return Ok(vec![]);
    }

    let mut msgs = Vec::with_capacity(elem_count.min(1000));
    let first = first_mono_time.get_or_insert(event_mono_time);
    let time_offset_ns = (event_mono_time as i64) - (*first as i64);
    let timestamp = *base_time + chrono::Duration::nanoseconds(time_offset_ns);

    for i in 0..elem_count {
        let elem_offset = list_start + 8 + i * elem_size * 8;
        if elem_offset + 16 > segment.len() {
            break;
        }
        // CanData: address @0 (4 bytes), busTime @1 (2 bytes), src @3 (1 byte)
        // Then pointer to dat @2 in pointer section
        let address = u32::from_le_bytes([
            segment[elem_offset],
            segment[elem_offset + 1],
            segment[elem_offset + 2],
            segment[elem_offset + 3],
        ]);
        let src = segment.get(elem_offset + 6).copied().unwrap_or(0);
        let dat_ptr_offset = elem_offset + 8; // Pointer section starts after 1 word data
        let dat = if struct_ptr_words > 0 && dat_ptr_offset + 8 <= segment.len() {
            let dat_ptr = read_u64_le(segment, dat_ptr_offset);
            decode_data_blob(segment, dat_ptr_offset / 8, dat_ptr)?
        } else {
            vec![]
        };

        if !dat.is_empty() || address != 0 {
            msgs.push(CanMessage {
                timestamp,
                bus: src,
                id: address,
                data: dat.into(),
            });
        }
    }

    Ok(msgs)
}

fn decode_data_blob(segment: &[u8], ptr_word_index: usize, ptr: u64) -> Result<Vec<u8>> {
    if (ptr & 3) != 1 {
        return Ok(vec![]);
    }
    let elem_type = ((ptr >> 32) & 7) as u8;
    let count = ((ptr >> 35) & 0x1FFF_FFFF) as usize;
    if elem_type != 2 {
        return Ok(vec![]); // Not byte list
    }
    let offset = ((ptr >> 2) & 0x3FFF_FFFF) as i32;
    let list_start = (ptr_word_index + 1 + offset as usize) * 8;
    if list_start + count > segment.len() {
        return Ok(vec![]);
    }
    Ok(segment[list_start..list_start + count].to_vec())
}

/// Extract (prefix, segment_num) for sort: "2025-06-25--21-24-49--12" -> ("2025-06-25--21-24-49", 12)
fn segment_sort_key(name: &str) -> (String, u32) {
    let parts: Vec<&str> = name.rsplitn(2, "--").collect();
    let suffix = parts.first().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0);
    let prefix = parts.get(1).unwrap_or(&name).to_string();
    (prefix, suffix)
}

fn read_u64_le(data: &[u8], offset: usize) -> u64 {
    if offset + 8 > data.len() {
        return 0;
    }
    u64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}
