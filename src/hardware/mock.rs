use async_trait::async_trait;
use crate::core::CanMessage;
use crate::hardware::can_interface::{CanInterface, CanConfig, CanStatus, CanResult, InterfaceType, InterfaceInfo};
use std::collections::VecDeque;
use chrono::Utc;

/// Mock CAN interface for testing without hardware
///
/// This interface simulates CAN traffic by generating random messages
/// or playing back recorded messages.
pub struct MockCanInterface {
    name: String,
    status: CanStatus,
    config: Option<CanConfig>,
    rx_buffer: VecDeque<CanMessage>,
    tx_buffer: VecDeque<CanMessage>,
    message_counter: u32,
    auto_generate: bool,
    bus_id: u8,
}

impl MockCanInterface {
    /// Create a new mock interface (defaults to bus 0)
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CanStatus::Disconnected,
            config: None,
            rx_buffer: VecDeque::new(),
            tx_buffer: VecDeque::new(),
            message_counter: 0,
            auto_generate: false,
            bus_id: 0,
        }
    }

    /// Create a new mock interface with a specific bus ID
    pub fn new_with_bus(name: &str, bus_id: u8) -> Self {
        Self {
            name: name.to_string(),
            status: CanStatus::Disconnected,
            config: None,
            rx_buffer: VecDeque::new(),
            tx_buffer: VecDeque::new(),
            message_counter: 0,
            auto_generate: false,
            bus_id,
        }
    }

    /// Enable automatic message generation
    pub fn set_auto_generate(&mut self, enabled: bool) {
        self.auto_generate = enabled;
    }

    /// Add a message to the receive buffer (for testing)
    pub fn inject_message(&mut self, message: CanMessage) {
        self.rx_buffer.push_back(message);
    }

    /// Add multiple messages to the receive buffer
    pub fn inject_messages(&mut self, messages: Vec<CanMessage>) {
        for msg in messages {
            self.rx_buffer.push_back(msg);
        }
    }

    /// Get all transmitted messages (for verification)
    pub fn take_sent_messages(&mut self) -> Vec<CanMessage> {
        self.tx_buffer.drain(..).collect()
    }

    /// Generate a simulated CAN message
    fn generate_message(&mut self) -> CanMessage {
        self.message_counter += 1;

        // Generate a message with some pattern
        let id = 0x100 + (self.message_counter % 10) as u32;
        let data = vec![
            (self.message_counter & 0xFF) as u8,
            ((self.message_counter >> 8) & 0xFF) as u8,
            ((self.message_counter >> 16) & 0xFF) as u8,
            ((self.message_counter >> 24) & 0xFF) as u8,
            0xDE,
            0xAD,
            0xBE,
            0xEF,
        ];

        CanMessage::new(self.bus_id, id, data.into())
    }
}

#[async_trait]
impl CanInterface for MockCanInterface {
    fn name(&self) -> &str {
        &self.name
    }

    fn status(&self) -> CanStatus {
        self.status
    }

    async fn connect(&mut self, config: CanConfig) -> CanResult<()> {
        self.config = Some(config);
        self.status = CanStatus::Connected;
        self.message_counter = 0;
        Ok(())
    }

    async fn disconnect(&mut self) -> CanResult<()> {
        self.status = CanStatus::Disconnected;
        self.config = None;
        self.rx_buffer.clear();
        self.tx_buffer.clear();
        Ok(())
    }

    async fn send(&mut self, message: &CanMessage) -> CanResult<()> {
        if self.status != CanStatus::Connected {
            return Err("Not connected".into());
        }
        self.tx_buffer.push_back(message.clone());
        Ok(())
    }

    async fn receive(&mut self) -> CanResult<Option<CanMessage>> {
        if self.status != CanStatus::Connected {
            return Err("Not connected".into());
        }

        // Generate a message if auto-generate is enabled and buffer is empty
        if self.auto_generate && self.rx_buffer.is_empty() {
            let msg = self.generate_message();
            self.rx_buffer.push_back(msg);
        }

        Ok(self.rx_buffer.pop_front())
    }

    fn rx_buffer_size(&self) -> usize {
        self.rx_buffer.len()
    }

    fn clear_rx_buffer(&mut self) {
        self.rx_buffer.clear();
    }

    fn supports_fd(&self) -> bool {
        true
    }
}

/// List available mock interfaces
pub fn list_mock_interfaces() -> Vec<InterfaceInfo> {
    vec![InterfaceInfo {
        name: "mock0".to_string(),
        interface_type: InterfaceType::Virtual,
        description: Some("Virtual CAN interface for testing".to_string()),
        available: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_interface_connect() {
        let mut iface = MockCanInterface::new("test");
        assert_eq!(iface.status(), CanStatus::Disconnected);

        iface.connect(CanConfig::default()).await.unwrap();
        assert_eq!(iface.status(), CanStatus::Connected);

        iface.disconnect().await.unwrap();
        assert_eq!(iface.status(), CanStatus::Disconnected);
    }

    #[tokio::test]
    async fn test_mock_interface_send_receive() {
        let mut iface = MockCanInterface::new("test");
        iface.connect(CanConfig::default()).await.unwrap();

        // Inject a message
        let msg = CanMessage::new(0, 0x123, crate::core::CanData::from_slice(&[1, 2, 3, 4]));
        iface.inject_message(msg.clone());

        // Receive the message
        let received = iface.receive().await.unwrap();
        assert!(received.is_some());
        let received = received.unwrap();
        assert_eq!(received.id, 0x123);
        assert_eq!(received.data, vec![1, 2, 3, 4]);

        // Buffer should be empty now
        assert_eq!(iface.rx_buffer_size(), 0);
    }

    #[tokio::test]
    async fn test_mock_interface_auto_generate() {
        let mut iface = MockCanInterface::new("test");
        iface.set_auto_generate(true);
        iface.connect(CanConfig::default()).await.unwrap();

        // Should generate messages
        let msg = iface.receive().await.unwrap();
        assert!(msg.is_some());

        let msg = iface.receive().await.unwrap();
        assert!(msg.is_some());
    }
}
