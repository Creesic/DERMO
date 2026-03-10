//! UDS Diagnostic Plugin
//!
//! Sends UDS (Unified Diagnostic Services, ISO 14229) requests to the engine ECU
//! over CAN via ISO-TP. Supports Read Data by Identifier (0x22) and Read DTC
//! Information (0x19).

use crate::plugins::isotp::{queue_isotp_send, IsotpReassembler};
use crate::plugins::Plugin;
use imgui::{Condition, Ui};

// UDS service IDs
const UDS_READ_DATA_BY_ID: u8 = 0x22;
const UDS_READ_DTC_INFO: u8 = 0x19;
const UDS_CLEAR_DTC: u8 = 0x14;
const UDS_TESTER_PRESENT: u8 = 0x3E;
const UDS_RESP_READ_DATA: u8 = 0x62;
const UDS_RESP_READ_DTC: u8 = 0x59;
const UDS_RESP_CLEAR_DTC: u8 = 0x54;

// Read DTC sub-functions
const DTC_REPORT_NUMBER_OF_DTC: u8 = 0x01;
const DTC_REPORT_DTC_BY_STATUS_MASK: u8 = 0x02;
const DTC_REPORT_DTC_SNAPSHOT_RECORD: u8 = 0x04;

// Common DIDs (Data Identifiers)
const DID_VIN: u16 = 0xF190;
const DID_CALIBRATION_ID: u16 = 0xF191;
const DID_ECU_SOFTWARE_VERSION: u16 = 0xF194;
const DID_ECU_HARDWARE_NUMBER: u16 = 0xF197;
/// OBD-II / manufacturer-specific (seen in BMW capture on 0x0DC)
const DID_OBD_MANUFACTURER: u16 = 0x07D0;

/// Preset DID for quick access
struct DidPreset {
    id: u16,
    name: &'static str,
}

const DID_PRESETS: &[DidPreset] = &[
    DidPreset {
        id: DID_VIN,
        name: "VIN (0xF190)",
    },
    DidPreset {
        id: DID_CALIBRATION_ID,
        name: "Calibration ID (0xF191)",
    },
    DidPreset {
        id: DID_ECU_SOFTWARE_VERSION,
        name: "ECU Software (0xF194)",
    },
    DidPreset {
        id: DID_ECU_HARDWARE_NUMBER,
        name: "ECU Hardware (0xF197)",
    },
    DidPreset {
        id: DID_OBD_MANUFACTURER,
        name: "OBD/Manufacturer (0x07D0)",
    },
];

/// Response IDs seen on Bus 0 (ECU → scanner) from capture
const BUS0_RESP_IDS: &[u32] = &[
    0x601, 0x60E, 0x617, 0x618, 0x62A, 0x64D, 0x64E, 0x65D, 0x65E,
];
/// Response IDs seen on Bus 1 from capture
const BUS1_RESP_IDS: &[u32] = &[0x598, 0x5E0, 0x5ED, 0x5EE, 0x5F2, 0x5F8, 0x5F9];
/// Combined for 0x6F4 broadcast (responses on both buses)
const BROADCAST_RESP_IDS: &[u32] = &[
    0x601, 0x60E, 0x617, 0x618, 0x62A, 0x64D, 0x64E, 0x65D, 0x65E,
    0x598, 0x5E0, 0x5ED, 0x5EE, 0x5F2, 0x5F8, 0x5F9,
];

/// Preset (req_id, resp_ids) for different vehicle/OBD setups
struct IdPreset {
    req: u32,
    resp_ids: &'static [u32],
    name: &'static str,
    /// When true, Clear DTC sends on 0x6F4 to all connected buses
    broadcast_clear: bool,
}

const ID_PRESETS: &[IdPreset] = &[
    IdPreset {
        req: 0x7E0,
        resp_ids: &[0x7E8],
        name: "OBD-II (7E0/7E8)",
        broadcast_clear: false,
    },
    IdPreset {
        req: 0x191,
        resp_ids: BUS0_RESP_IDS,
        name: "BMW Bus 0 (191)",
        broadcast_clear: false,
    },
    IdPreset {
        req: 0x193,
        resp_ids: BUS0_RESP_IDS,
        name: "BMW Bus 0 (193)",
        broadcast_clear: false,
    },
    IdPreset {
        req: 0x0DC,
        resp_ids: BUS1_RESP_IDS,
        name: "BMW Bus 1 (0DC/598)",
        broadcast_clear: false,
    },
    IdPreset {
        req: 0x6F4,
        resp_ids: BROADCAST_RESP_IDS,
        name: "BMW Clear DTC (6F4)",
        broadcast_clear: true,
    },
];

pub struct UdsDiagPlugin {
    tx_bus: u8,
    req_id: u32,
    resp_ids: Vec<u32>,
    resp_ids_edit: String,
    broadcast_clear: bool,
    custom_did_hex: String,
    isotp_rx: IsotpReassembler,
    last_response: Option<Vec<u8>>,
    last_response_hex: String,
    last_response_text: String,
}

impl UdsDiagPlugin {
    pub fn new() -> Self {
        Self {
            tx_bus: 0,
            req_id: 0x7E0,
            resp_ids: vec![0x7E8],
            resp_ids_edit: "7E8".to_string(),
            broadcast_clear: false,
            custom_did_hex: "F190".to_string(),
            isotp_rx: IsotpReassembler::new(),
            last_response: None,
            last_response_hex: String::new(),
            last_response_text: String::new(),
        }
    }

    fn parse_did(s: &str) -> Option<u16> {
        let s = s.trim().trim_start_matches("0x").replace(' ', "");
        if s.len() != 4 {
            return None;
        }
        u16::from_str_radix(&s, 16).ok()
    }

    fn process_message(&mut self, msg: &crate::hardware::can_manager::ManagerMessage) {
        let bus_ok = self.broadcast_clear || msg.message.bus == self.tx_bus;
        let id_ok = self.resp_ids.contains(&msg.message.id);
        if !bus_ok || !id_ok {
            return;
        }
        if let Some(payload) = self.isotp_rx.feed(&msg.message.data) {
            self.last_response = Some(payload.clone());
            self.last_response_hex = payload.iter().map(|b| format!("{:02X}", b)).collect();

            // Parse response for display
            if payload.len() >= 3 {
                let svc = payload[0];
                if svc == UDS_RESP_READ_DATA {
                    // 0x62 DID_hi DID_lo data...
                    let did = u16::from_be_bytes([payload[1], payload[2]]);
                    let data = &payload[3..];
                    self.last_response_text = format_response_data(did, data);
                } else if svc == 0x7F {
                    // Negative response
                    let req_svc = payload.get(1).copied().unwrap_or(0);
                    let nrc = payload.get(2).copied().unwrap_or(0);
                    self.last_response_text = format!(
                        "Negative: req=0x{:02X} NRC=0x{:02X}",
                        req_svc, nrc
                    );
                } else if svc == UDS_RESP_CLEAR_DTC {
                    self.last_response_text = "Clear DTC: OK".to_string();
                } else if svc == UDS_RESP_READ_DTC && payload.len() >= 4 {
                    // 0x59 subfn status_mask DTC_count [DTCs...]
                    let subfn = payload[1];
                    let status_mask = payload[2];
                    let dtc_count = payload[3];
                    let mut s = format!(
                        "DTC subfn=0x{:02X} mask=0x{:02X} count={}",
                        subfn, status_mask, dtc_count
                    );
                    if payload.len() >= 4 + dtc_count as usize * 4 {
                        let dtcs = parse_dtcs(&payload[4..4 + dtc_count as usize * 4]);
                        for dtc in dtcs {
                            s.push_str(&format!("\n  {}", dtc));
                        }
                    }
                    self.last_response_text = s;
                } else {
                    self.last_response_text = format!("Raw: {}", self.last_response_hex);
                }
            } else {
                self.last_response_text = format!("Raw: {}", self.last_response_hex);
            }
        }
    }

    fn queue_read_did(&self, ctx: &mut crate::plugins::PluginContext, did: u16) {
        let payload = vec![
            UDS_READ_DATA_BY_ID,
            (did >> 8) as u8,
            (did & 0xFF) as u8,
        ];
        queue_isotp_send(ctx, self.tx_bus, self.req_id, &payload);
    }

    fn queue_read_dtc_by_status(&self, ctx: &mut crate::plugins::PluginContext, mask: u8) {
        let payload = vec![UDS_READ_DTC_INFO, DTC_REPORT_DTC_BY_STATUS_MASK, mask];
        queue_isotp_send(ctx, self.tx_bus, self.req_id, &payload);
    }

    fn queue_read_dtc_count(&self, ctx: &mut crate::plugins::PluginContext) {
        let payload = vec![UDS_READ_DTC_INFO, DTC_REPORT_NUMBER_OF_DTC];
        queue_isotp_send(ctx, self.tx_bus, self.req_id, &payload);
    }

    /// 0x19 0x04 Report DTC snapshot - seen in BMW capture with mask 0x7F0006
    fn queue_read_dtc_snapshot(&self, ctx: &mut crate::plugins::PluginContext) {
        let payload = vec![
            UDS_READ_DTC_INFO,
            DTC_REPORT_DTC_SNAPSHOT_RECORD,
            0x7F,
            0x00,
            0x06,
            0x00,
        ];
        queue_isotp_send(ctx, self.tx_bus, self.req_id, &payload);
    }

    fn queue_tester_present(&self, ctx: &mut crate::plugins::PluginContext) {
        let payload = vec![UDS_TESTER_PRESENT, 0x00];
        queue_isotp_send(ctx, self.tx_bus, self.req_id, &payload);
    }

    fn queue_clear_dtc(&self, ctx: &mut crate::plugins::PluginContext) {
        let payload = vec![UDS_CLEAR_DTC, 0xFF, 0xFF];
        if self.broadcast_clear && self.req_id == 0x6F4 {
            for &bus in ctx.connected_buses {
                queue_isotp_send(ctx, bus, 0x6F4, &payload);
            }
        } else {
            queue_isotp_send(ctx, self.tx_bus, self.req_id, &payload);
        }
    }
}

fn format_response_data(did: u16, data: &[u8]) -> String {
    match did {
        DID_VIN => {
            // VIN is typically 17 ASCII chars
            let s: String = data
                .iter()
                .take(17)
                .filter_map(|&b| {
                    if (0x20..=0x7E).contains(&b) {
                        Some(b as char)
                    } else {
                        None
                    }
                })
                .collect();
            format!("VIN: {}", s)
        }
        DID_OBD_MANUFACTURER => {
            let hex: String = data.iter().map(|b| format!("{:02X} ", b)).collect();
            let ascii: String = data
                .iter()
                .filter_map(|&b| {
                    if (0x20..=0x7E).contains(&b) {
                        Some(b as char)
                    } else {
                        None
                    }
                })
                .collect();
            format!("0x07D0: {} | ASCII: {}", hex.trim(), ascii)
        }
        DID_CALIBRATION_ID | DID_ECU_SOFTWARE_VERSION | DID_ECU_HARDWARE_NUMBER => {
            let s: String = data
                .iter()
                .filter_map(|&b| {
                    if (0x20..=0x7E).contains(&b) && b != 0 {
                        Some(b as char)
                    } else if b == 0 {
                        None
                    } else {
                        Some('?')
                    }
                })
                .collect();
            format!("{}", s)
        }
        _ => {
            let hex: String = data.iter().map(|b| format!("{:02X} ", b)).collect();
            format!("DID 0x{:04X}: {}", did, hex.trim())
        }
    }
}

fn parse_dtcs(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in data.chunks(4) {
        if chunk.len() >= 3 {
            // ISO 14229 DTC: 3 bytes (DTCHigh, DTCMid, DTCLow), 4th byte = status
            let b0 = chunk[0];
            let b1 = chunk[1];
            let b2 = chunk[2];
            let dtc_type = (b0 >> 4) & 0x03;
            let type_char = match dtc_type {
                0 => 'P',
                1 => 'C',
                2 => 'B',
                3 => 'U',
                _ => '?',
            };
            let code = format!(
                "{}{:X}{:X}{:X}{:X}",
                type_char,
                b0 & 0x0F,
                (b1 >> 4) & 0x0F,
                b1 & 0x0F,
                (b2 >> 4) & 0x0F
            );
            out.push(code);
        }
    }
    out
}

impl Plugin for UdsDiagPlugin {
    fn id(&self) -> &str {
        "uds_diag"
    }

    fn name(&self) -> &str {
        "UDS Diagnostic"
    }

    fn description(&self) -> &str {
        "Request info from engine ECU via UDS (Read DID 0x22, Read DTCs 0x19)"
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

        ui.window("UDS Diagnostic")
            .size([520.0, 420.0], Condition::FirstUseEver)
            .position([120.0, 140.0], Condition::FirstUseEver)
            .opened(is_open)
            .build(|| {
                if !ctx.is_connected && !ctx.has_playback {
                    ui.text_colored([1.0, 0.5, 0.3, 1.0], "No CAN interface connected");
                    ui.text_wrapped("Connect to CAN to send UDS requests to the engine ECU.");
                    return;
                }
                if ctx.has_playback && !ctx.is_connected {
                    ui.text_colored([0.5, 0.8, 0.5, 1.0], "Playback mode (read-only)");
                    ui.separator();
                }

                if ctx.is_connected {
                    ui.text("ID preset:");
                    ui.same_line();
                    for preset in ID_PRESETS {
                        if ui.button(preset.name) {
                            self.req_id = preset.req;
                            self.resp_ids = preset.resp_ids.to_vec();
                            self.resp_ids_edit = self
                                .resp_ids
                                .iter()
                                .map(|id| format!("{:03X}", id))
                                .collect::<Vec<_>>()
                                .join(", ");
                            self.broadcast_clear = preset.broadcast_clear;
                        }
                        ui.same_line();
                    }
                    ui.new_line();

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
                        if let Ok(n) =
                            u32::from_str_radix(req_hex.trim().trim_start_matches("0x"), 16)
                        {
                            self.req_id = n;
                        }
                    }

                    ui.text("Response IDs:");
                    ui.same_line();
                    if ui.input_text("##resp_ids", &mut self.resp_ids_edit).build() {
                        let ids: Vec<u32> = self
                            .resp_ids_edit
                            .split(',')
                            .filter_map(|s| {
                                u32::from_str_radix(
                                    s.trim().trim_start_matches("0x"),
                                    16,
                                )
                                .ok()
                            })
                            .collect();
                        if !ids.is_empty() {
                            self.resp_ids = ids;
                        }
                    }
                    if !ui.is_item_focused() {
                        self.resp_ids_edit = self
                            .resp_ids
                            .iter()
                            .map(|id| format!("{:03X}", id))
                            .collect::<Vec<_>>()
                            .join(", ");
                    }
                    if ui.is_item_hovered() {
                        ui.tooltip(|| {
                            ui.text_wrapped("Comma-separated hex IDs (e.g. 601, 617, 618)");
                        });
                    }

                    ui.separator();
                }

                ui.text("Read Data by Identifier (0x22)");
                ui.separator();

                for preset in DID_PRESETS {
                    if ui.button(preset.name) {
                        self.queue_read_did(ctx, preset.id);
                    }
                    ui.same_line();
                }
                ui.new_line();

                ui.text("Custom DID (hex):");
                ui.same_line();
                if ui.input_text("##did", &mut self.custom_did_hex)
                    .hint("F190")
                    .build()
                {}
                ui.same_line();
                if ui.button("Read DID") {
                    if let Some(did) = Self::parse_did(&self.custom_did_hex) {
                        self.queue_read_did(ctx, did);
                    }
                }

                ui.separator();
                ui.text("Read DTC Information (0x19)");
                ui.separator();

                if ui.button("Report DTC count (0x01)") {
                    self.queue_read_dtc_count(ctx);
                }
                ui.same_line();
                if ui.button("Report DTCs (0x02, mask 0xFF)") {
                    self.queue_read_dtc_by_status(ctx, 0xFF);
                }
                ui.same_line();
                if ui.button("DTC snapshot (0x04, 7F0006)") {
                    self.queue_read_dtc_snapshot(ctx);
                }
                if ui.is_item_hovered() {
                    ui.tooltip(|| {
                        ui.text_wrapped("Report DTC snapshot (BMW capture pattern)");
                    });
                }

                ui.text("Other:");
                ui.same_line();
                if ui.button("Tester Present (0x3E)") {
                    self.queue_tester_present(ctx);
                }
                ui.same_line();
                if ui.button("Clear DTC (0x14)") {
                    self.queue_clear_dtc(ctx);
                }
                if ui.is_item_hovered() {
                    ui.tooltip(|| {
                        ui.text_wrapped(if self.broadcast_clear {
                            "Clear DTC broadcast on 0x6F4 to all buses"
                        } else {
                            "Clear all DTCs (0x14 0xFF 0xFF)"
                        });
                    });
                }

                ui.separator();
                ui.text("Last response");
                ui.separator();

                if !self.last_response_text.is_empty() {
                    ui.text_wrapped(&self.last_response_text);
                }
                if !self.last_response_hex.is_empty() {
                    ui.text_disabled(&self.last_response_hex);
                }

                ui.separator();
                ui.text_disabled("UDS ISO 14229 over ISO-TP. Use ID presets for BMW/capture testing.");
            });
    }
}
