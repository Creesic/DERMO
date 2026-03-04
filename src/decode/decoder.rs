use crate::core::dbc::{DbcFile, DbcMessage, DbcSignal, ByteOrder, ValueType};
use crate::core::CanMessage;
use chrono::{DateTime, Utc};

/// A decoded signal value from a CAN message
#[derive(Debug, Clone)]
pub struct DecodedSignal {
    /// Signal name
    pub name: String,
    /// Signal value (physical value after factor/offset)
    pub physical_value: f64,
    /// Raw value (before factor/offset)
    pub raw_value: u64,
    /// Signal unit
    pub unit: Option<String>,
    /// Message timestamp
    pub timestamp: DateTime<Utc>,
    /// Message ID this came from
    pub message_id: u32,
}

/// Signal decoder that extracts signals from CAN messages using DBC definitions
pub struct SignalDecoder {
    dbc: Option<DbcFile>,
}

impl SignalDecoder {
    pub fn new() -> Self {
        Self { dbc: None }
    }

    pub fn set_dbc(&mut self, dbc: DbcFile) {
        self.dbc = Some(dbc);
    }

    pub fn clear_dbc(&mut self) {
        self.dbc = None;
    }

    /// Decode all signals from a CAN message
    pub fn decode_message(&self, msg: &CanMessage) -> Vec<DecodedSignal> {
        let dbc = match &self.dbc {
            Some(dbc) => dbc,
            None => return Vec::new(),
        };

        let dbc_msg = match dbc.get_message(msg.id) {
            Some(m) => m,
            None => return Vec::new(),
        };

        dbc_msg.signals.iter()
            .filter_map(|signal| self.decode_signal(msg, signal))
            .collect()
    }

    /// Decode a single signal from a CAN message
    pub fn decode_signal(&self, msg: &CanMessage, signal: &DbcSignal) -> Option<DecodedSignal> {
        let raw_value = extract_bits(&msg.data, signal.start_bit, signal.bit_length, signal.byte_order)?;

        // Apply sign extension for signed values
        let raw_value = if signal.value_type == ValueType::Signed {
            sign_extend(raw_value, signal.bit_length)
        } else {
            raw_value
        };

        // Apply factor and offset to get physical value
        let physical_value = (raw_value as f64) * signal.factor + signal.offset;

        Some(DecodedSignal {
            name: signal.name.clone(),
            physical_value,
            raw_value,
            unit: signal.unit.clone(),
            timestamp: msg.timestamp,
            message_id: msg.id,
        })
    }

    /// Encode a signal value into CAN data bytes
    pub fn encode_signal(&self, data: &mut [u8], signal: &DbcSignal, physical_value: f64) -> bool {
        // Convert physical value to raw value
        let raw_value = ((physical_value - signal.offset) / signal.factor) as i64;

        // Convert to unsigned for bit manipulation
        let raw_unsigned = if raw_value < 0 {
            // Handle negative values
            let mask = (1u64 << signal.bit_length) - 1;
            (raw_value as u64) & mask
        } else {
            raw_value as u64
        };

        insert_bits(data, raw_unsigned, signal.start_bit, signal.bit_length, signal.byte_order)
    }
}

impl Default for SignalDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract bits from a byte array
///
/// # Arguments
/// * `data` - The CAN message data bytes
/// * `start_bit` - Starting bit position (0-63, in DBC notation)
/// * `bit_length` - Number of bits to extract
/// * `byte_order` - Intel (little-endian) or Motorola (big-endian)
pub fn extract_bits(data: &[u8], start_bit: u8, bit_length: u8, byte_order: ByteOrder) -> Option<u64> {
    if data.is_empty() || bit_length == 0 || bit_length > 64 {
        return None;
    }

    let start_bit = start_bit as usize;
    let bit_length = bit_length as usize;

    // Convert DBC bit position to actual bit position
    let (byte_idx, bit_idx) = match byte_order {
        ByteOrder::Intel => {
            // Intel: bits are numbered LSB first within bytes, sequential across bytes
            // Bit N is at byte (N / 8), bit position (N % 8)
            (start_bit / 8, start_bit % 8)
        }
        ByteOrder::Motorola => {
            // Motorola: bits are numbered MSB first within bytes
            // DBC uses a confusing numbering scheme for Motorola
            // Bit N in DBC notation maps to byte (N / 8), bit position (7 - (N % 8))
            // But for multi-byte signals, the bytes are reversed
            dbc_motorola_to_position(start_bit)
        }
    };

    if byte_idx >= data.len() {
        return None;
    }

    // Read the value byte by byte
    let mut result: u64 = 0;
    let mut bits_remaining = bit_length;
    let mut current_byte = byte_idx;
    let mut current_bit = bit_idx;

    while bits_remaining > 0 && current_byte < data.len() {
        let bits_to_read = bits_remaining.min(8 - current_bit);
        // Use u32 for the mask calculation to avoid overflow when bits_to_read is 8
        let mask = (((1u32 << bits_to_read) - 1) << current_bit) as u8;
        let bits = ((data[current_byte] & mask) >> current_bit) as u64;

        let shift = (bit_length - bits_remaining) as u32;
        result |= bits << shift;

        bits_remaining -= bits_to_read;
        current_bit += bits_to_read;
        if current_bit >= 8 {
            current_bit = 0;
            current_byte += 1;
        }
    }

    Some(result)
}

/// Convert DBC Motorola bit position to byte/bit position
///
/// In DBC format, Motorola signals use a special bit numbering:
/// - Byte 0: bits 7,6,5,4,3,2,1,0 (MSB to LSB)
/// - Byte 1: bits 15,14,13,12,11,10,9,8
/// etc.
fn dbc_motorola_to_position(dbc_bit: usize) -> (usize, usize) {
    let byte = dbc_bit / 8;
    let bit_in_byte = 7 - (dbc_bit % 8);
    (byte, bit_in_byte)
}

/// Insert bits into a byte array
pub fn insert_bits(data: &mut [u8], value: u64, start_bit: u8, bit_length: u8, byte_order: ByteOrder) -> bool {
    if data.is_empty() || bit_length == 0 || bit_length > 64 {
        return false;
    }

    let start_bit = start_bit as usize;
    let bit_length = bit_length as usize;

    let (byte_idx, bit_idx) = match byte_order {
        ByteOrder::Intel => (start_bit / 8, start_bit % 8),
        ByteOrder::Motorola => dbc_motorola_to_position(start_bit),
    };

    if byte_idx >= data.len() {
        return false;
    }

    let mut bits_remaining = bit_length;
    let mut current_byte = byte_idx;
    let mut current_bit = bit_idx;
    let mut value_shift = 0u32;

    while bits_remaining > 0 && current_byte < data.len() {
        let bits_to_write = bits_remaining.min(8 - current_bit);
        let mask = ((1u64 << bits_to_write) - 1) << value_shift;
        let bits = ((value & mask) >> value_shift) as u8;

        // Use u32 for clear_mask calculation to avoid overflow when bits_to_write is 8
        let clear_mask = !((((1u32 << bits_to_write) - 1) << current_bit) as u8);
        data[current_byte] = (data[current_byte] & clear_mask) | (bits << current_bit);

        bits_remaining -= bits_to_write;
        value_shift += bits_to_write as u32;
        current_bit += bits_to_write;
        if current_bit >= 8 {
            current_bit = 0;
            current_byte += 1;
        }
    }

    true
}

/// Sign extend a value to 64 bits
fn sign_extend(value: u64, bit_length: u8) -> u64 {
    if bit_length >= 64 {
        return value;
    }

    let sign_bit = 1u64 << (bit_length - 1);
    if value & sign_bit != 0 {
        // Negative value - extend the sign
        let mask = !((1u64 << bit_length) - 1);
        value | mask
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_bits_intel_single_byte() {
        let data = [0b11010010u8];
        // Extract bits 2-5 (4 bits starting at bit 2)
        let result = extract_bits(&data, 2, 4, ByteOrder::Intel);
        assert_eq!(result, Some(0b0100)); // bits 2-5 are 0100
    }

    #[test]
    fn test_extract_bits_intel_full_byte() {
        let data = [0xABu8];
        let result = extract_bits(&data, 0, 8, ByteOrder::Intel);
        assert_eq!(result, Some(0xAB));
    }

    #[test]
    fn test_extract_bits_intel_multi_byte() {
        let data = [0xCDu8, 0xABu8];
        // Little-endian: 0xABCD = 0xCD at byte 0, 0xAB at byte 1
        let result = extract_bits(&data, 0, 16, ByteOrder::Intel);
        assert_eq!(result, Some(0xABCD));
    }

    #[test]
    fn test_insert_bits_intel() {
        let mut data = [0u8, 0u8];
        insert_bits(&mut data, 0xABCD, 0, 16, ByteOrder::Intel);
        assert_eq!(data[0], 0xCD);
        assert_eq!(data[1], 0xAB);
    }

    #[test]
    fn test_sign_extend_positive() {
        let result = sign_extend(5, 4); // 0101 in 4 bits
        // This is positive, should not change
        assert_eq!(result as i64, 5);
    }

    #[test]
    fn test_sign_extend_negative() {
        let result = sign_extend(0b1111, 4) as i64; // -1 in 4-bit two's complement
        assert_eq!(result, -1);
    }

    #[test]
    fn test_decode_signal() {
        let mut dbc = DbcFile::new();
        dbc.add_message(DbcMessage {
            id: 0x123,
            name: "TestMessage".to_string(),
            size: 8,
            signals: vec![DbcSignal {
                name: "TestSignal".to_string(),
                start_bit: 0,
                bit_length: 8,
                byte_order: ByteOrder::Intel,
                value_type: ValueType::Unsigned,
                factor: 0.5,
                offset: -40.0,
                minimum: None,
                maximum: None,
                unit: Some("degC".to_string()),
                multiplexor: None,
            }],
        });

        let decoder = SignalDecoder::new();
        let mut decoder = decoder;
        decoder.set_dbc(dbc);

        let msg = CanMessage::new(0, 0x123, crate::core::CanData::from_slice(&[100]));
        let signals = decoder.decode_message(&msg);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].name, "TestSignal");
        assert_eq!(signals[0].raw_value, 100);
        assert_eq!(signals[0].physical_value, 10.0); // 100 * 0.5 - 40 = 10
    }

    #[test]
    fn test_insert_and_extract_roundtrip() {
        let mut data = [0u8; 8];

        // Test various bit positions and lengths
        let test_cases = [
            (0u8, 8u8, 0xABu64),   // First byte
            (8, 8, 0xCDu64),       // Second byte
            (4, 12, 0xABCu64),     // Crossing byte boundary
            (16, 16, 0x1234u64),   // Two bytes
        ];

        for (start, len, value) in test_cases {
            data.fill(0);
            insert_bits(&mut data, value, start, len, ByteOrder::Intel);
            let result = extract_bits(&data, start, len, ByteOrder::Intel);
            assert_eq!(result, Some(value), "Failed for start={}, len={}", start, len);
        }
    }
}
