use async_trait::async_trait;
use crate::core::CanMessage;
use crate::hardware::can_interface::{CanInterface, CanConfig, CanStatus, CanResult, InterfaceType, InterfaceInfo};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::SerialPortBuilderExt;
use tokio::sync::mpsc;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn, error};

/// Buffer size for received messages
const RX_BUFFER_SIZE: usize = 10000;

/// SLCAN/Lawicel protocol serial CAN interface
///
/// Supports common USB-CAN adapters that use the SLCAN protocol:
/// - CANtact
/// - CANable
/// - Lawicel CANUSB
/// - Various USB-CAN adapters
pub struct SerialCanInterface {
    /// Interface name (serial port path)
    name: String,
    /// Current status
    status: CanStatus,
    /// Serial port handle
    port: Option<tokio_serial::SerialStream>,
    /// Configuration
    config: Option<CanConfig>,
    /// Receive buffer
    rx_buffer: VecDeque<CanMessage>,
    /// RX buffer size counter for atomic access
    rx_count: Arc<AtomicUsize>,
    /// TX channel for sending messages to the serial task
    tx_sender: Option<mpsc::Sender<Vec<u8>>>,
    /// Line buffer for accumulating partial SLCAN frames
    line_buffer: String,
    /// Bus ID for this interface
    bus_id: u8,
}

impl SerialCanInterface {
    /// Create a new serial CAN interface (defaults to bus 0)
    pub fn new(port_name: &str) -> Self {
        debug!("Creating new SerialCanInterface for port: {}", port_name);
        Self {
            name: port_name.to_string(),
            status: CanStatus::Disconnected,
            port: None,
            config: None,
            rx_buffer: VecDeque::with_capacity(RX_BUFFER_SIZE),
            rx_count: Arc::new(AtomicUsize::new(0)),
            tx_sender: None,
            line_buffer: String::new(),
            bus_id: 0,
        }
    }

    /// Create a new serial CAN interface with a specific bus ID
    pub fn new_with_bus(port_name: &str, bus_id: u8) -> Self {
        debug!("Creating new SerialCanInterface for port: {} with bus_id: {}", port_name, bus_id);
        Self {
            name: port_name.to_string(),
            status: CanStatus::Disconnected,
            port: None,
            config: None,
            rx_buffer: VecDeque::with_capacity(RX_BUFFER_SIZE),
            rx_count: Arc::new(AtomicUsize::new(0)),
            tx_sender: None,
            line_buffer: String::new(),
            bus_id,
        }
    }

    /// List available serial ports that might be CAN interfaces
    pub fn list_serial_ports() -> Vec<String> {
        let ports = tokio_serial::available_ports()
            .unwrap_or_default();
        eprintln!("[S.H.I.T] Found {} serial ports:", ports.len());
        for p in &ports {
            eprintln!("[S.H.I.T]   - {}", p.port_name);
        }
        ports.into_iter()
            .map(|p| p.port_name)
            .collect()
    }

    /// Build SLCAN command to set bitrate
    fn build_bitrate_command(bitrate: u32) -> Vec<u8> {
        // SLCAN bitrate codes
        let code = match bitrate {
            10_000 => '0',
            20_000 => '1',
            50_000 => '2',
            100_000 => '3',
            125_000 => '4',
            250_000 => '5',
            500_000 => '6',
            800_000 => '7',
            1_000_000 => '8',
            _ => '6', // Default to 500k
        };
        format!("S{}\r", code).into_bytes()
    }

    /// Build SLCAN command to open CAN channel
    fn build_open_command(listen_only: bool) -> Vec<u8> {
        if listen_only {
            b"L\r".to_vec()  // Open in listen-only mode
        } else {
            b"O\r".to_vec()  // Open in normal mode
        }
    }

    /// Build SLCAN command to close CAN channel
    fn build_close_command() -> Vec<u8> {
        b"C\r".to_vec()
    }

    /// Parse an SLCAN frame into a CAN message
    fn parse_frame(&self, line: &str) -> Option<CanMessage> {
        if line.is_empty() {
            return None;
        }

        let frame_type = line.chars().next()?;
        let data = line.get(1..)?;

        match frame_type {
            // Standard CAN frame (11-bit ID)
            't' => Self::parse_standard_frame(data, false, self.bus_id),
            // Extended CAN frame (29-bit ID)
            'T' => Self::parse_extended_frame(data, false, self.bus_id),
            // Standard RTR frame
            'r' => Self::parse_standard_frame(data, true, self.bus_id),
            // Extended RTR frame
            'R' => Self::parse_extended_frame(data, true, self.bus_id),
            _ => None,
        }
    }

    /// Parse a standard (11-bit ID) CAN frame
    fn parse_standard_frame(data: &str, _is_rtr: bool, bus_id: u8) -> Option<CanMessage> {
        // Format: TIIIDDDDDDDDDDD (ID = 3 hex chars, DLC = 1 hex char, Data = 0-16 hex chars)
        if data.len() < 4 {
            return None;
        }

        let id = u32::from_str_radix(&data[0..3], 16).ok()?;
        let dlc = data[3..4].parse::<usize>().ok()?;

        let expected_len = 4 + dlc * 2;
        if data.len() < expected_len {
            return None;
        }

        let hex_data = &data[4..expected_len];
        let msg_data = Self::parse_hex_data(hex_data)?;

        Some(CanMessage::new(bus_id, id, msg_data.into()))
    }

    /// Parse an extended (29-bit ID) CAN frame
    fn parse_extended_frame(data: &str, _is_rtr: bool, bus_id: u8) -> Option<CanMessage> {
        // Format: TIIIIIIIIDDDDDDDDDDD (ID = 8 hex chars, DLC = 1 hex char, Data = 0-16 hex chars)
        if data.len() < 9 {
            return None;
        }

        let id = u32::from_str_radix(&data[0..8], 16).ok()?;
        let dlc = data[8..9].parse::<usize>().ok()?;

        let expected_len = 9 + dlc * 2;
        if data.len() < expected_len {
            return None;
        }

        let hex_data = &data[9..expected_len];
        let msg_data = Self::parse_hex_data(hex_data)?;

        Some(CanMessage::new(bus_id, id, msg_data.into()))
    }

    /// Parse hex data string into bytes
    fn parse_hex_data(hex: &str) -> Option<Vec<u8>> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
            .collect()
    }

    /// Build an SLCAN command to transmit a CAN frame
    fn build_tx_command(message: &CanMessage) -> Vec<u8> {
        let dlc = message.data.len();
        let data_hex: String = message.data.iter()
            .map(|b| format!("{:02X}", b))
            .collect();

        if message.is_extended() {
            // Extended frame: TIIIIIIIIDDDDDDDDDDD
            format!("T{:08X}{}{}\r", message.id, dlc, data_hex).into_bytes()
        } else {
            // Standard frame: tIIIDDDDDDDDDDD
            format!("t{:03X}{}{}\r", message.id, dlc, data_hex).into_bytes()
        }
    }

    /// Send a command and wait for SLCAN acknowledgment (\r)
    async fn send_command_wait_ack(port: &mut tokio_serial::SerialStream, cmd: &[u8]) -> CanResult<()> {
        eprintln!("[CAN-Viz SLCAN] Sending command: {:?} ({})", cmd, String::from_utf8_lossy(cmd));
        debug!("Sending SLCAN command: {}", String::from_utf8_lossy(cmd));

        port.write_all(cmd).await
            .map_err(|e| {
                eprintln!("[CAN-Viz SLCAN] Write failed: {}", e);
                format!("Failed to write command: {}", e)
            })?;
        port.flush().await
            .map_err(|e| {
                eprintln!("[CAN-Viz SLCAN] Flush failed: {}", e);
                format!("Failed to flush command: {}", e)
            })?;
        eprintln!("[CAN-Viz SLCAN] Command sent, waiting for ACK...");

        // Wait for ACK (carriage return '\r') with timeout
        let mut buf = [0u8; 128];
        let deadline = tokio::time::sleep(Duration::from_millis(500));
        tokio::pin!(deadline);

        let response_start = std::time::Instant::now();

        loop {
            tokio::select! {
                _ = &mut deadline => {
                    let elapsed = response_start.elapsed().as_millis();
                    eprintln!("[CAN-Viz SLCAN] TIMEOUT after {}ms - no ACK from device!", elapsed);
                    eprintln!("[CAN-Viz SLCAN] Command was: {:?}", String::from_utf8_lossy(cmd));
                    warn!("SLCAN command timeout (no ACK after {}ms): {}",
                          elapsed,
                          String::from_utf8_lossy(cmd));
                    return Err(format!("Command timeout - no ACK from device for: {}",
                                      String::from_utf8_lossy(cmd)).into());
                }
                result = port.read(&mut buf) => {
                    match result {
                        Ok(0) => {
                            // Keep waiting
                            continue;
                        }
                        Ok(n) => {
                            let response = String::from_utf8_lossy(&buf[..n]);
                            eprintln!("[CAN-Viz SLCAN] Received {} bytes: {:?}", n, response);
                            debug!("Received response: {:?}", response);

                            // Check for ACK (carriage return)
                            if response.contains('\r') {
                                eprintln!("[CAN-Viz SLCAN] ACK received (carriage return found)");
                                debug!("ACK received for command: {}", String::from_utf8_lossy(cmd));
                                return Ok(());
                            }
                            // Some devices send additional data, check if we got \r
                            for byte in &buf[..n] {
                                if *byte == b'\r' {
                                    eprintln!("[CAN-Viz SLCAN] ACK received (byte scan found \\r)");
                                    debug!("ACK received for command: {}", String::from_utf8_lossy(cmd));
                                    return Ok(());
                                }
                            }
                        }
                        Err(e) => {
                            error!("Read error while waiting for ACK: {}", e);
                            return Err(format!("Read error: {}", e).into());
                        }
                    }
                }
            }
        }
    }
}

#[async_trait]
impl CanInterface for SerialCanInterface {
    fn name(&self) -> &str {
        &self.name
    }

    fn status(&self) -> CanStatus {
        self.status
    }

    async fn connect(&mut self, config: CanConfig) -> CanResult<()> {
        eprintln!("[CAN-Viz SerialCan] Connecting to: {} at bitrate: {}", self.name, config.bitrate);
        info!("Connecting to serial port: {} at bitrate: {}", self.name, config.bitrate);

        // Open serial port
        eprintln!("[CAN-Viz SerialCan] Opening serial port at 1,000,000 baud...");
        let mut port = tokio_serial::new(&self.name, 1_000_000)  // SLCAN standard baud rate
            .timeout(Duration::from_millis(100))
            .open_native_async()
            .map_err(|e| {
                eprintln!("[CAN-Viz SerialCan] FAILED to open port: {}", e);
                format!("Failed to open serial port {}: {}", self.name, e)
            })?;
        eprintln!("[CAN-Viz SerialCan] Serial port opened successfully!");

        // Clear any pending data in the buffer and see what's there
        let mut junk_buf = [0u8; 256];
        let mut total_cleared = 0;
        loop {
            match port.try_read(&mut junk_buf) {
                Ok(n) if n > 0 => {
                    total_cleared += n;
                    eprintln!("[CAN-Viz SerialCan] Cleared {} bytes from buffer: {:02X?}", n, &junk_buf[..n]);
                }
                _ => break,
            }
        }
        if total_cleared > 0 {
            eprintln!("[CAN-Viz SerialCan] Total {} bytes cleared from buffer", total_cleared);
        }

        // Send a close command first to ensure any previous session is terminated
        eprintln!("[CAN-Viz SerialCan] Sending close command 'C' to reset device state...");
        let _ = port.write_all(b"C\r").await;
        let _ = port.flush().await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Clear any response from the close command
        let mut clear_buf = [0u8; 256];
        let _ = tokio::time::timeout(Duration::from_millis(50), port.read(&mut clear_buf)).await;
        eprintln!("[CAN-Viz SerialCan] Device reset complete");

        // Check if device might be candleLight firmware by looking for its heartbeat/announcement
        // candleLight devices typically send a version string on startup
        eprintln!("[CAN-Viz SerialCan] Checking for candleLight firmware...");

        // Try to read any initial announcement with a short timeout
        let mut init_buf = [0u8; 256];
        match tokio::time::timeout(Duration::from_millis(100), port.read(&mut init_buf)).await {
            Ok(Ok(n)) if n > 0 => {
                eprintln!("[CAN-Viz SerialCan] Device sent {} bytes on open: {:02X?}", n, &init_buf[..n]);
                if let Ok(s) = std::str::from_utf8(&init_buf[..n]) {
                    eprintln!("[CAN-Viz SerialCan] As string: {:?}", s);
                    if s.contains("candle") || s.contains("CANDLE") || s.contains("slcan") || s.contains("SLCAN") {
                        eprintln!("[CAN-Viz SerialCan] Detected device identification!");
                    }
                }
            }
            _ => {
                eprintln!("[CAN-Viz SerialCan] No initial data from device");
            }
        }

        // Try to send a version command ('V') - both slcan and candleLight should respond
        eprintln!("[CAN-Viz SerialCan] Sending version command 'V' to probe firmware...");
        let _ = port.write_all(b"V\r").await;
        let _ = port.flush().await;

        // Read the ENTIRE version response until we get the '\r' terminator
        let mut ver_data = Vec::new();
        let mut ver_buf = [0u8; 256];
        let read_deadline = tokio::time::sleep(Duration::from_millis(200));
        tokio::pin!(read_deadline);
        let mut got_cr = false;
        loop {
            tokio::select! {
                _ = &mut read_deadline => {
                    break;
                }
                result = port.read(&mut ver_buf) => {
                    match result {
                        Ok(0) => { /* keep waiting */ }
                        Ok(n) => {
                            ver_data.extend_from_slice(&ver_buf[..n]);
                            eprintln!("[CAN-Viz SerialCan] Read {} bytes of version data", n);
                            // Check if we got \r
                            for &byte in &ver_buf[..n] {
                                if byte == b'\r' {
                                    got_cr = true;
                                }
                            }
                            if got_cr {
                                eprintln!("[CAN-Viz SerialCan] Got complete version response (CR found)");
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }
        if !ver_data.is_empty() {
            eprintln!("[CAN-Viz SerialCan] Version response {} bytes: {:02X?}", ver_data.len(), ver_data);
            if let Ok(s) = std::str::from_utf8(&ver_data) {
                eprintln!("[CAN-Viz SerialCan] Version string: {:?}", s);
            }
        } else {
            eprintln!("[CAN-Viz SerialCan] No response to version command");
        }

        // Clear any remaining data in the buffer before sending bitrate command
        eprintln!("[CAN-Viz SerialCan] Clearing any remaining buffer data...");
        let mut clear_buf = [0u8; 256];
        let mut clear_count = 0;
        loop {
            match port.try_read(&mut clear_buf) {
                Ok(n) if n > 0 => {
                    clear_count += n;
                    eprintln!("[CAN-Viz SerialCan] Cleared {} more bytes: {:02X?}", n, &clear_buf[..n]);
                }
                _ => break,
            }
        }
        if clear_count > 0 {
            eprintln!("[CAN-Viz SerialCan] Total {} bytes cleared before bitrate command", clear_count);
        }

        // Send bitrate command and wait for ACK
        let bitrate_cmd = Self::build_bitrate_command(config.bitrate);
        eprintln!("[CAN-Viz SerialCan] Sending bitrate command: {:?}", String::from_utf8_lossy(&bitrate_cmd));

        // Try with ACK first, then try without if it times out
        let mut bitrate_success = false;
        match Self::send_command_wait_ack(&mut port, &bitrate_cmd).await {
            Ok(()) => {
                eprintln!("[CAN-Viz SerialCan] Bitrate command ACK received!");
                bitrate_success = true;
            }
            Err(_) => {
                eprintln!("[CAN-Viz SerialCan] Bitrate command timed out waiting for ACK, trying fire-and-forget mode...");
                // Try fire-and-forget mode
                let _ = port.write_all(&bitrate_cmd).await;
                let _ = port.flush().await;
                tokio::time::sleep(Duration::from_millis(50)).await;
                bitrate_success = true;
                eprintln!("[CAN-Viz SerialCan] Bitrate command sent (no ACK expected)");
            }
        }

        if !bitrate_success {
            return Err("Bitrate command failed".into());
        }

        info!("Bitrate set to {} bps", config.bitrate);

        // Small delay after bitrate configuration
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Open CAN channel - also try fire-and-forget if ACK fails
        let open_cmd = Self::build_open_command(config.listen_only);
        eprintln!("[CAN-Viz SerialCan] Sending open command: {:?}", String::from_utf8_lossy(&open_cmd));

        match Self::send_command_wait_ack(&mut port, &open_cmd).await {
            Ok(()) => {
                eprintln!("[CAN-Viz SerialCan] Open command ACK received!");
            }
            Err(_) => {
                eprintln!("[CAN-Viz SerialCan] Open command timed out waiting for ACK, trying fire-and-forget mode...");
                // Try fire-and-forget mode
                let _ = port.write_all(&open_cmd).await;
                let _ = port.flush().await;
                tokio::time::sleep(Duration::from_millis(50)).await;
                eprintln!("[CAN-Viz SerialCan] Open command sent (no ACK expected)");
            }
        }

        info!("CAN channel opened (listen_only: {})", config.listen_only);

        // Warm-up period: Give the device time to start receiving CAN messages
        // Some devices need a moment to initialize their CAN hardware
        eprintln!("[CAN-Viz SerialCan] Waiting for device to stabilize...");
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Verification: Try to read any initial data to verify the device is working
        // This helps catch devices that are connected but not actually receiving
        eprintln!("[CAN-Viz SerialCan] Verifying device is receiving data...");
        let mut test_buf = [0u8; 256];
        let verification_start = std::time::Instant::now();
        let mut received_any_data = false;
        let verification_timeout = Duration::from_millis(500);

        while verification_start.elapsed() < verification_timeout {
            match port.try_read(&mut test_buf) {
                Ok(n) if n > 0 => {
                    received_any_data = true;
                    let data_str = String::from_utf8_lossy(&test_buf[..n]);
                    eprintln!("[CAN-Viz SerialCan] Verification: Received {} bytes during warm-up: {:?}", n, data_str);

                    // Check if this looks like CAN messages (starts with t, T, r, or R)
                    if data_str.chars().any(|c| matches!(c, 't' | 'T' | 'r' | 'R')) {
                        eprintln!("[CAN-Viz SerialCan] Verification: Detected CAN message format - device is receiving!");
                        break;
                    }
                }
                _ => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }

        if received_any_data {
            eprintln!("[CAN-Viz SerialCan] Device verification: Data detected - connection healthy");
        } else {
            eprintln!("[CAN-Viz SerialCan] Device verification: No data during warm-up (this may be normal if no CAN traffic)");
        }

        // Final clear of any buffered data before handing off to the receive loop
        eprintln!("[CAN-Viz SerialCan] Final buffer clear before starting receive loop...");
        let mut final_buf = [0u8; 256];
        let mut final_clear_count = 0;
        loop {
            match port.try_read(&mut final_buf) {
                Ok(n) if n > 0 => {
                    final_clear_count += n;
                }
                _ => break,
            }
        }
        if final_clear_count > 0 {
            eprintln!("[CAN-Viz SerialCan] Cleared {} bytes from final buffer", final_clear_count);
        }

        self.port = Some(port);
        self.config = Some(config);
        self.status = CanStatus::Connected;
        self.line_buffer.clear();

        info!("Successfully connected to {}", self.name);
        Ok(())
    }

    async fn disconnect(&mut self) -> CanResult<()> {
        info!("Disconnecting from {}", self.name);

        if let Some(mut port) = self.port.take() {
            // Send close command
            let close_cmd = Self::build_close_command();
            debug!("Sending close command: {}", String::from_utf8_lossy(&close_cmd));
            let _ = port.write_all(&close_cmd).await;
            let _ = port.flush().await;
        }

        self.status = CanStatus::Disconnected;
        self.config = None;
        self.rx_buffer.clear();
        self.rx_count.store(0, Ordering::SeqCst);
        self.line_buffer.clear();

        info!("Disconnected from {}", self.name);
        Ok(())
    }

    async fn send(&mut self, message: &CanMessage) -> CanResult<()> {
        let port = self.port.as_mut().ok_or("Not connected")?;

        let cmd = Self::build_tx_command(message);
        port.write_all(&cmd).await?;
        port.flush().await?;

        Ok(())
    }

    async fn receive(&mut self) -> CanResult<Option<CanMessage>> {
        // First return any buffered messages
        if let Some(msg) = self.rx_buffer.pop_front() {
            self.rx_count.fetch_sub(1, Ordering::SeqCst);
            return Ok(Some(msg));
        }

        // Try to read more data from the port
        if let Some(port) = self.port.as_mut() {
            let mut buf = [0u8; 256];

            // Use blocking read with timeout instead of try_read
            // Increased timeout to 200ms for better reliability with slower devices
            match tokio::time::timeout(
                Duration::from_millis(200),
                port.read(&mut buf)
            ).await {
                Ok(Ok(0)) => {
                    // Empty read, nothing to do
                }
                Ok(Ok(n)) => {
                    let data = &buf[..n];
                    debug!("Received {} bytes from serial port", n);

                    // Accumulate data in line buffer
                    if let Ok(text) = std::str::from_utf8(data) {
                        self.line_buffer.push_str(text);

                        // Process complete lines (SLCAN frames end with \r)
                        while let Some(cr_pos) = self.line_buffer.find('\r') {
                            let line = self.line_buffer[..cr_pos].trim().to_string();
                            // Remove the processed line including the \r
                            self.line_buffer = self.line_buffer[cr_pos + 1..].to_string();

                            if !line.is_empty() {
                                debug!("Processing SLCAN line: {:?}", line);
                                if let Some(msg) = self.parse_frame(&line) {
                                    debug!("Parsed CAN message: ID=0x{:03X}, len={}",
                                           msg.id, msg.data.len());
                                    if self.rx_buffer.len() < RX_BUFFER_SIZE {
                                        self.rx_buffer.push_back(msg);
                                        self.rx_count.fetch_add(1, Ordering::SeqCst);
                                    }
                                } else {
                                    warn!("Failed to parse SLCAN frame: {:?}", line);
                                }
                            }
                        }

                        // Also handle \n line endings for compatibility
                        while let Some(lf_pos) = self.line_buffer.find('\n') {
                            let line = self.line_buffer[..lf_pos].trim().to_string();
                            self.line_buffer = self.line_buffer[lf_pos + 1..].to_string();

                            if !line.is_empty() {
                                debug!("Processing SLCAN line (LF): {:?}", line);
                                if let Some(msg) = self.parse_frame(&line) {
                                    debug!("Parsed CAN message: ID=0x{:03X}, len={}",
                                           msg.id, msg.data.len());
                                    if self.rx_buffer.len() < RX_BUFFER_SIZE {
                                        self.rx_buffer.push_back(msg);
                                        self.rx_count.fetch_add(1, Ordering::SeqCst);
                                    }
                                }
                            }
                        }
                    } else {
                        warn!("Received non-UTF8 data: {:?}", data);
                    }
                }
                Ok(Err(e)) => {
                    error!("Serial port read error: {}", e);
                    return Err(format!("Read error: {}", e).into());
                }
                Err(_) => {
                    // Timeout, no data available
                }
            }
        }

        // Return a message from the buffer if we added any
        let msg = self.rx_buffer.pop_front();
        if msg.is_some() {
            self.rx_count.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(msg)
    }

    fn rx_buffer_size(&self) -> usize {
        self.rx_count.load(Ordering::SeqCst)
    }

    fn clear_rx_buffer(&mut self) {
        self.rx_buffer.clear();
        self.rx_count.store(0, Ordering::SeqCst);
    }

    fn supports_fd(&self) -> bool {
        false  // Basic SLCAN doesn't support CAN FD
    }
}

/// List all available serial CAN interfaces
pub fn list_interfaces() -> Vec<InterfaceInfo> {
    SerialCanInterface::list_serial_ports()
        .into_iter()
        .map(|name| InterfaceInfo {
            name: name.clone(),
            interface_type: InterfaceType::Serial,
            description: Some(format!("Serial port: {}", name)),
            available: true,
        })
        .collect()
}
