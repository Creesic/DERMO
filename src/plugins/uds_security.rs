//! UDS RSA Security Key Plugin
//!
//! Implements the RSA Security Key Algorithm for UDS 0x27 Security Access.
//! Input: ECU ID (2 bytes from SVK offset 0x1C-0x1D), Seed (8 bytes from UDS 0x67 0x11).
//! Process: MD5 buffer → integer → RSA encrypt → format with chunk reversal and header.

use crate::plugins::isotp::{queue_isotp_send, IsotpReassembler};
use crate::plugins::Plugin;
use imgui::{Condition, Ui};
use num_bigint::BigUint;
use num_traits::{Num, Zero};
use std::io::{Read, Seek};

const UDS_SVC_SECURITY_ACCESS: u8 = 0x27;
const UDS_REQ_SEED: u8 = 0x11;
const UDS_SEND_KEY: u8 = 0x12;
const UDS_RESP_SEED: u8 = 0x67;

/// Gen1: 512-bit RSA, 64-byte key. Gen2: 1024-bit RSA, 128-byte key.
#[derive(Clone, Copy, PartialEq)]
enum Gen {
    Gen1,
    Gen2,
}

impl Gen {
    fn key_bytes(&self) -> usize {
        match self {
            Gen::Gen1 => 64,
            Gen::Gen2 => 128,
        }
    }
    fn header(&self) -> [u8; 4] {
        match self {
            Gen::Gen1 => [0x00, 0x00, 0x00, 0x10],
            Gen::Gen2 => [0x00, 0x00, 0x00, 0x20],
        }
    }
}

/// Compute the RSA Security Key from ECU ID and seed.
fn compute_security_key(
    ecu_id: [u8; 2],
    seed: [u8; 8],
    gen: Gen,
    n: &BigUint,
    e: &BigUint,
) -> Result<Vec<u8>, String> {
    // 1. Build 16-byte buffer: [0xFF 0xFF 0xFF 0xFF 0x00 0x00 ECU_ID 8-byte-seed]
    let mut buffer = [0u8; 16];
    buffer[0..4].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    buffer[4..6].copy_from_slice(&[0x00, 0x00]);
    buffer[6..8].copy_from_slice(&ecu_id);
    buffer[8..16].copy_from_slice(&seed);

    // 2. MD5 hash → 16 bytes
    let digest = md5::compute(&buffer);
    let md5_result: [u8; 16] = digest.0;

    // 3. Convert MD5 to integer (little-endian)
    let m = BigUint::from_bytes_le(&md5_result);

    // 4. RSA encrypt: c = m^e mod n
    let c = m.modpow(e, n);

    // 5. Convert to key_bytes (big-endian), pad if needed
    let key_len = gen.key_bytes();
    let mut raw = c.to_bytes_be();
    if raw.len() > key_len {
        return Err("RSA result too large".to_string());
    }
    while raw.len() < key_len {
        raw.insert(0, 0);
    }

    // 6. Reverse in 4-byte chunks (chunk order reversed, bytes in chunk unchanged)
    let mut reversed = Vec::with_capacity(key_len);
    for chunk in raw.chunks(4).rev() {
        reversed.extend_from_slice(chunk);
    }

    // 7. Prepend header
    let mut result = Vec::with_capacity(4 + key_len);
    result.extend_from_slice(&gen.header());
    result.extend(reversed);

    Ok(result)
}

pub struct UdsSecurityPlugin {
    ecu_id: [u8; 2],
    seed: [u8; 8],
    gen: Gen,
    rsa_n_hex: String,
    rsa_e_hex: String,
    computed_key: Option<Vec<u8>>,
    last_error: Option<String>,
    tx_bus: u8,
    req_id: u32,
    resp_id: u32,
    last_seed_from_can: Option<[u8; 8]>,
    isotp_rx: IsotpReassembler,
}

impl UdsSecurityPlugin {
    pub fn new() -> Self {
        Self {
            ecu_id: [0, 0],
            seed: [0, 0, 0, 0, 0, 0, 0, 0],
            gen: Gen::Gen1,
            rsa_n_hex: String::new(),
            rsa_e_hex: "10001".to_string(),
            computed_key: None,
            last_error: None,
            tx_bus: 0,
            req_id: 0x7E0,
            resp_id: 0x7E8,
            last_seed_from_can: None,
            isotp_rx: IsotpReassembler::new(),
        }
    }

    fn parse_hex_2(s: &str) -> Option<[u8; 2]> {
        let s = s.trim().trim_start_matches("0x").replace(' ', "");
        if s.len() != 4 {
            return None;
        }
        let a = u8::from_str_radix(&s[0..2], 16).ok()?;
        let b = u8::from_str_radix(&s[2..4], 16).ok()?;
        Some([a, b])
    }

    fn parse_hex_8(s: &str) -> Option<[u8; 8]> {
        let s = s.trim().trim_start_matches("0x").replace(' ', "");
        if s.len() != 16 {
            return None;
        }
        let mut out = [0u8; 8];
        for i in 0..8 {
            out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(out)
    }

    fn load_svk_ecu_id(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("SVK / binary", &["bin", "svk", "dat", "*"])
            .pick_file()
        {
            if let Ok(mut f) = std::fs::File::open(&path) {
                let _ = f.seek(std::io::SeekFrom::Start(0x1C));
                let mut buf = [0u8; 2];
                if f.read_exact(&mut buf).is_ok() {
                    self.ecu_id = buf;
                }
            }
        }
    }

    fn process_message(&mut self, msg: &crate::hardware::can_manager::ManagerMessage) {
        if msg.message.id != self.resp_id {
            return;
        }
        if let Some(payload) = self.isotp_rx.feed(&msg.message.data) {
            if payload.len() >= 10 && payload[0] == UDS_RESP_SEED && payload[1] == UDS_REQ_SEED {
                let mut seed = [0u8; 8];
                seed.copy_from_slice(&payload[2..10]);
                self.last_seed_from_can = Some(seed);
            }
        }
    }

    fn do_compute(&mut self) {
        self.last_error = None;
        self.computed_key = None;

        let n_hex = self.rsa_n_hex.trim().trim_start_matches("0x").replace(' ', "");
        let n: BigUint = match BigUint::from_str_radix(&n_hex, 16) {
            Ok(v) if !v.is_zero() => v,
            _ => {
                self.last_error = Some("Invalid or empty RSA modulus (n)".to_string());
                return;
            }
        };

        let e_hex = self.rsa_e_hex.trim().trim_start_matches("0x").replace(' ', "");
        let e: BigUint = match BigUint::from_str_radix(&e_hex, 16) {
            Ok(v) if !v.is_zero() => v,
            _ => {
                self.last_error = Some("Invalid or empty RSA exponent (e)".to_string());
                return;
            }
        };

        match compute_security_key(self.ecu_id, self.seed, self.gen, &n, &e) {
            Ok(key) => self.computed_key = Some(key),
            Err(e) => self.last_error = Some(e),
        }
    }

    fn queue_request_seed(&self, ctx: &mut crate::plugins::PluginContext) {
        let payload = vec![UDS_SVC_SECURITY_ACCESS, UDS_REQ_SEED];
        queue_isotp_send(ctx, self.tx_bus, self.req_id, &payload);
    }

    fn queue_send_key(&self, ctx: &mut crate::plugins::PluginContext) {
        let Some(ref key) = self.computed_key else { return };
        let mut payload = vec![UDS_SVC_SECURITY_ACCESS, UDS_SEND_KEY];
        payload.extend_from_slice(key);
        queue_isotp_send(ctx, self.tx_bus, self.req_id, &payload);
    }
}

impl Plugin for UdsSecurityPlugin {
    fn id(&self) -> &str {
        "uds_security"
    }

    fn name(&self) -> &str {
        "UDS RSA Security Key"
    }

    fn description(&self) -> &str {
        "UDS 0x27 Security Access - RSA key from ECU ID + seed (MD5, Gen1/Gen2)"
    }

    fn render(
        &mut self,
        ui: &Ui,
        ctx: &mut crate::plugins::PluginContext,
        messages: &[crate::hardware::can_manager::ManagerMessage],
        is_open: &mut bool,
    ) {
        for msg in messages {
            self.process_message(msg);
        }

        ui.window("UDS RSA Security Key")
            .size([480.0, 520.0], Condition::FirstUseEver)
            .position([100.0, 120.0], Condition::FirstUseEver)
            .opened(is_open)
            .build(|| {
                if !ctx.is_connected && !ctx.has_playback {
                    ui.text_colored([1.0, 0.5, 0.3, 1.0], "No CAN interface connected");
                    ui.text_wrapped("Connect to CAN or open a log. Sending UDS requires a live connection.");
                    return;
                }
                if ctx.has_playback && !ctx.is_connected {
                    ui.text_colored([0.5, 0.8, 0.5, 1.0], "Playback mode");
                    ui.separator();
                }

                if ctx.is_connected {
                    ui.text("TX Bus:");
                    ui.same_line();
                    let bus_labels: Vec<String> =
                        ctx.connected_buses.iter().map(|b| format!("Bus {}", b)).collect();
                    if !bus_labels.is_empty() {
                        let mut bus_idx = ctx
                            .connected_buses
                            .iter()
                            .position(|&b| b == self.tx_bus)
                            .unwrap_or(0);
                        if bus_idx >= bus_labels.len() {
                            bus_idx = 0;
                        }
                        let labels_ref: Vec<&str> =
                            bus_labels.iter().map(|s| s.as_str()).collect();
                        if ui.combo_simple_string("##tx_bus", &mut bus_idx, &labels_ref) {
                            if let Some(&bus) = ctx.connected_buses.get(bus_idx) {
                                self.tx_bus = bus;
                            }
                        }
                    }

                    ui.text("Request ID:");
                    ui.same_line();
                    let mut req_hex = format!("{:03X}", self.req_id);
                    if ui.input_text("##req_id", &mut req_hex).build() {
                        if let Ok(n) = u32::from_str_radix(req_hex.trim().trim_start_matches("0x"), 16) {
                            self.req_id = n;
                        }
                    }

                    ui.text("Response ID:");
                    ui.same_line();
                    let mut resp_hex = format!("{:03X}", self.resp_id);
                    if ui.input_text("##resp_id", &mut resp_hex).build() {
                        if let Ok(n) = u32::from_str_radix(resp_hex.trim().trim_start_matches("0x"), 16) {
                            self.resp_id = n;
                        }
                    }

                    ui.separator();
                }

                ui.text("Inputs");
                ui.separator();

                ui.text("ECU ID (2 bytes, hex):");
                ui.same_line();
                let mut ecu_hex = format!("{:02X}{:02X}", self.ecu_id[0], self.ecu_id[1]);
                if ui.input_text("##ecu_id", &mut ecu_hex).build() {
                    if let Some(id) = Self::parse_hex_2(&ecu_hex) {
                        self.ecu_id = id;
                    }
                }
                ui.same_line();
                if ui.button("Load from SVK") {
                    self.load_svk_ecu_id();
                }
                if ui.is_item_hovered() {
                    ui.tooltip(|| {
                        ui.text_wrapped("Reads 2 bytes at offset 0x1C from selected file.");
                    });
                }

                ui.text("Seed (8 bytes, hex):");
                ui.same_line();
                let mut seed_hex = self
                    .seed
                    .iter()
                    .map(|b| format!("{:02X}", b))
                    .collect::<String>();
                if ui.input_text("##seed", &mut seed_hex).build() {
                    if let Some(s) = Self::parse_hex_8(&seed_hex) {
                        self.seed = s;
                    }
                }
                if let Some(s) = self.last_seed_from_can {
                    ui.same_line();
                    if ui.button("Use from CAN") {
                        self.seed = s;
                    }
                    if ui.is_item_hovered() {
                        ui.tooltip(|| {
                            ui.text_wrapped("Use seed from last UDS 0x67 0x11 response.");
                        });
                    }
                }

                ui.separator();
                ui.text("Algorithm");
                ui.separator();

                ui.text("Gen:");
                ui.same_line();
                let mut gen_idx = if self.gen == Gen::Gen1 { 0 } else { 1 };
                let gen_names = ["Gen1 (512-bit)", "Gen2 (1024-bit)"];
                if ui.combo_simple_string("##gen", &mut gen_idx, &gen_names) {
                    self.gen = if gen_idx == 0 { Gen::Gen1 } else { Gen::Gen2 };
                }

                ui.text("RSA n (hex):");
                if ui.input_text("##rsa_n", &mut self.rsa_n_hex)
                    .hint("modulus")
                    .build()
                {}

                ui.text("RSA e (hex):");
                if ui.input_text("##rsa_e", &mut self.rsa_e_hex)
                    .hint("65537 = 10001")
                    .build()
                {}

                ui.separator();

                if ui.button("Compute Key") {
                    self.do_compute();
                }

                if ctx.is_connected {
                    ui.same_line();
                    if ui.button("Request Seed") {
                        self.queue_request_seed(ctx);
                    }
                    if ui.is_item_hovered() {
                        ui.tooltip(|| {
                            ui.text_wrapped("Send UDS 0x27 0x11 via ISO-TP.");
                        });
                    }

                    if self.computed_key.is_some() {
                        ui.same_line();
                        if ui.button("Send Key") {
                            self.queue_send_key(ctx);
                        }
                        if ui.is_item_hovered() {
                            ui.tooltip(|| {
                                ui.text_wrapped("Send UDS 0x27 0x12 with computed key via ISO-TP.");
                            });
                        }
                    }
                }

                if let Some(ref err) = self.last_error {
                    ui.text_colored([1.0, 0.3, 0.3, 1.0], err);
                }

                if let Some(ref key) = self.computed_key {
                    ui.separator();
                    ui.text("Computed Key:");
                    let hex: String = key.iter().map(|b| format!("{:02X}", b)).collect();
                    ui.text_wrapped(&hex);
                }

                ui.separator();
                ui.text_disabled("UDS 0x27 Security Access, MD5+RSA, ISO-TP");
            });
    }
}
