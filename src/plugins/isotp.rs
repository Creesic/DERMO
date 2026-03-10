//! ISO-TP (ISO 15765-2) transport for UDS over CAN
//!
//! Reassembles single/multi-frame ISO-TP payloads and queues sends.

use crate::core::CanMessage;
use crate::plugins::PluginContext;

/// ISO-TP single frame: 0x0N + data (N = length, max 7 bytes payload)
fn is_isotp_single(data: &[u8]) -> Option<usize> {
    if data.is_empty() {
        return None;
    }
    let pci = data[0];
    let pci_type = (pci >> 4) & 0x0F;
    if pci_type == 0x0 {
        Some((pci & 0x0F) as usize)
    } else {
        None
    }
}

/// ISO-TP first frame: 0x1FFF + 6 bytes (length 12 bits, 6 data)
fn is_isotp_first(data: &[u8]) -> Option<(usize, &[u8])> {
    if data.len() < 2 {
        return None;
    }
    let pci = data[0];
    let pci_type = (pci >> 4) & 0x0F;
    if pci_type == 0x1 {
        let len = ((data[0] & 0x0F) as usize) << 8 | data[1] as usize;
        let payload = if data.len() > 2 { &data[2..] } else { &[] };
        Some((len, payload))
    } else {
        None
    }
}

/// ISO-TP consecutive frame: 0x2N + 7 bytes
fn is_isotp_consecutive(data: &[u8]) -> Option<&[u8]> {
    if data.is_empty() {
        return None;
    }
    let pci_type = (data[0] >> 4) & 0x0F;
    if pci_type == 0x2 {
        Some(if data.len() > 1 { &data[1..] } else { &[] })
    } else {
        None
    }
}

/// Reassemble ISO-TP payload from single or multi-frame
pub struct IsotpReassembler {
    total_len: usize,
    buffer: Vec<u8>,
    expecting_consecutive: bool,
}

impl IsotpReassembler {
    pub fn new() -> Self {
        Self {
            total_len: 0,
            buffer: Vec::new(),
            expecting_consecutive: false,
        }
    }

    pub fn feed(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        if data.is_empty() {
            return None;
        }

        if self.expecting_consecutive {
            if let Some(payload) = is_isotp_consecutive(data) {
                self.buffer.extend_from_slice(payload);
                if self.buffer.len() >= self.total_len {
                    self.expecting_consecutive = false;
                    let result = self.buffer.clone();
                    self.buffer.clear();
                    self.total_len = 0;
                    return Some(result);
                }
            }
            return None;
        }

        if let Some(len) = is_isotp_single(data) {
            if data.len() >= 1 + len {
                return Some(data[1..1 + len].to_vec());
            }
        }

        if let Some((len, payload)) = is_isotp_first(data) {
            self.total_len = len;
            self.buffer = payload.to_vec();
            if self.buffer.len() >= len {
                self.expecting_consecutive = false;
                let result = self.buffer.clone();
                self.buffer.clear();
                self.total_len = 0;
                return Some(result);
            } else {
                self.expecting_consecutive = true;
            }
        }

        None
    }

    pub fn reset(&mut self) {
        self.total_len = 0;
        self.buffer.clear();
        self.expecting_consecutive = false;
    }
}

impl Default for IsotpReassembler {
    fn default() -> Self {
        Self::new()
    }
}

/// Build ISO-TP frames for payload and queue as CAN messages
pub fn queue_isotp_send(ctx: &mut PluginContext, bus: u8, can_id: u32, payload: &[u8]) {
    if payload.len() <= 7 {
        let mut data = vec![0x0u8 | (payload.len() as u8)];
        data.extend_from_slice(payload);
        ctx.queue_send
            .push((bus, CanMessage::new(bus, can_id, data.into())));
        return;
    }

    let len = payload.len();
    let mut data = vec![0x10, (len >> 8) as u8, (len & 0xFF) as u8];
    data.extend_from_slice(&payload[..6.min(len)]);
    ctx.queue_send
        .push((bus, CanMessage::new(bus, can_id, data.into())));

    let mut seq = 1u8;
    let mut offset = 6;
    while offset < len {
        let chunk_len = (len - offset).min(7);
        let mut frame = vec![0x20 | seq];
        frame.extend_from_slice(&payload[offset..offset + chunk_len]);
        ctx.queue_send
            .push((bus, CanMessage::new(bus, can_id, frame.into())));
        offset += chunk_len;
        seq = (seq + 1) & 0x0F;
    }
}
