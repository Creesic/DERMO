use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

/// Stack-allocated CAN data payload (0-8 bytes, no heap allocation).
///
/// CAN frames always carry 0-8 bytes. Using a fixed-size array avoids a heap
/// allocation per message — critical when loading logs with millions of messages.
/// Implements `Deref<Target=[u8]>` so `.len()`, `.iter()`, `.get()`, indexing,
/// and slice comparisons all work transparently.
#[derive(Clone, Copy)]
pub struct CanData {
    bytes: [u8; 8],
    len: u8,
}

impl CanData {
    /// Create an empty CAN data payload.
    pub fn new() -> Self {
        Self { bytes: [0; 8], len: 0 }
    }

    /// Create from a byte slice (truncates to 8 bytes).
    pub fn from_slice(data: &[u8]) -> Self {
        let mut bytes = [0u8; 8];
        let len = data.len().min(8);
        bytes[..len].copy_from_slice(&data[..len]);
        Self { bytes, len: len as u8 }
    }

    /// Append a byte (ignored if already at 8 bytes).
    pub fn push(&mut self, byte: u8) {
        if (self.len as usize) < 8 {
            self.bytes[self.len as usize] = byte;
            self.len += 1;
        }
    }

    /// Get the payload as a slice.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// Convert to a heap-allocated Vec (for APIs that require Vec<u8>).
    pub fn to_vec(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }
}

impl Default for CanData {
    fn default() -> Self {
        Self::new()
    }
}

impl std::ops::Deref for CanData {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl std::fmt::Debug for CanData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_slice())
    }
}

impl PartialEq for CanData {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl PartialEq<Vec<u8>> for CanData {
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl PartialEq<&[u8]> for CanData {
    fn eq(&self, other: &&[u8]) -> bool {
        self.as_slice() == *other
    }
}

impl From<Vec<u8>> for CanData {
    fn from(v: Vec<u8>) -> Self {
        Self::from_slice(&v)
    }
}

impl From<&[u8]> for CanData {
    fn from(s: &[u8]) -> Self {
        Self::from_slice(s)
    }
}

impl Serialize for CanData {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Serialize as a sequence of bytes for compatibility with Vec<u8> format
        self.as_slice().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CanData {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        Ok(CanData::from_slice(&bytes))
    }
}

/// A raw CAN message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanMessage {
    /// Timestamp in UTC
    pub timestamp: DateTime<Utc>,

    /// CAN bus ID (0, 1, 2, etc.)
    pub bus: u8,

    /// CAN message ID (11-bit or 29-bit)
    pub id: u32,

    /// Raw data bytes (0-8 bytes, stack-allocated)
    pub data: CanData,
}

impl CanMessage {
    /// Create a new CAN message
    pub fn new(bus: u8, id: u32, data: CanData) -> Self {
        Self {
            timestamp: Utc::now(),
            bus,
            id,
            data,
        }
    }

    /// Check if this is an extended (29-bit) CAN ID
    pub fn is_extended(&self) -> bool {
        self.id > 0x7FF
    }

    /// Get data as hex string
    pub fn hex_data(&self) -> String {
        self.data
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Get timestamp as Unix timestamp in seconds
    pub fn timestamp_unix(&self) -> f64 {
        self.timestamp.timestamp_millis() as f64 / 1000.0
    }

    /// Parse hex string to CAN data bytes
    pub fn parse_hex(hex: &str) -> anyhow::Result<CanData> {
        let hex = hex.replace(' ', "");
        // Strip 0x or 0X prefix if present
        let hex = hex.strip_prefix("0x").or_else(|| hex.strip_prefix("0X")).unwrap_or(&hex);

        if hex.len() % 2 != 0 {
            anyhow::bail!("Hex string must have even length");
        }

        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("Failed to parse hex: {}", e))?;

        Ok(CanData::from_slice(&bytes))
    }
}
