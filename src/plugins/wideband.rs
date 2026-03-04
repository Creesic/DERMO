//! rusEFI Wideband CAN Plugin
//!
//! Implements the rusEFI wideband protocol matching TunerStudio's "rusEFI Wideband Tools".
//! See rusefi_wideband_can_protocol.md and epicefi_fw firmware.
//!
//! Transmit (controller → ECU): WidebandStandardData, WidebandDiagData (RusEfi 100 Hz),
//! AEMNet UEGO (100 Hz), AEMNet EGT (20 Hz), Pong (on Ping).
//!
//! Receive (ECU → controller): SetIndex (DLC 1/2/3), WidebandControl, Ping, SetSensorType, HeaterConfig.
//! All commands use 29-bit extended IDs with header 0xEF. 500 kbps.

use crate::core::CanMessage;
use crate::plugins::Plugin;
use imgui::{Condition, Ui};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use tracing::debug;

// Protocol constants (from rusefi_wideband_can_protocol.md and rusefi_wideband.cpp)
const WB_ACK_ID: u32 = 0x727573;
const WB_CMD_ECU_STATUS: u32 = 0x0EF50000;
const WB_CMD_SET_INDEX: u32 = 0x0EF40000;
const WB_CMD_PING: u32 = 0x0EF60000;
const WB_CMD_SET_SENSOR_TYPE: u32 = 0x0EF70000;

/// HeaterConfig (0xEF80000) - heater thresholds, preheat time; DLC≥3. Byte layout TBD.
#[allow(dead_code)]
const WB_CMD_HEATER_CONFIG: u32 = 0x0EF80000;

// AEMNet (29-bit extended, big-endian)
const AEM_UEGO_BASE: u32 = 0x180;
const AEM_EGT_BASE: u32 = 0x0A0305;

/// Status codes from WBO::Status (wideband_can.h) - matches TunerStudio AfrFaultList/HeaterStatesList
const STATUS_PREHEAT: u8 = 0;
const STATUS_WARMUP: u8 = 1;
const STATUS_RUNNING_CLOSED_LOOP: u8 = 2;
const STATUS_SENSOR_DIDNT_HEAT: u8 = 3;
const STATUS_SENSOR_OVERHEAT: u8 = 4;
const STATUS_SENSOR_UNDERHEAT: u8 = 5;
const STATUS_NO_SUPPLY: u8 = 6;

/// Command status (matches canReWidebandCmdStatus)
#[derive(Clone, Copy, PartialEq)]
enum CmdStatus {
    Idle = 0,
    Done = 1,
    Busy = 2,
    Failed = 3,
}

/// Sensor types (matches canReWidebandSensorType)
const SENSOR_NAMES: [&str; 4] = [
    "Bosch LSU 4.9",
    "Bosch LSU 4.2",
    "Bosch LSU ADV",
    "FAE LSU 4.9",
];

/// Status names from TunerStudio ini (AfrFaultList / HeaterStatesList)
fn status_name(s: u8) -> &'static str {
    match s {
        STATUS_PREHEAT => "Preheat",
        STATUS_WARMUP => "Warmup",
        STATUS_RUNNING_CLOSED_LOOP => "Running",
        STATUS_SENSOR_DIDNT_HEAT => "Failed to heat",
        STATUS_SENSOR_OVERHEAT => "Overheat",
        STATUS_SENSOR_UNDERHEAT => "Underheat",
        STATUS_NO_SUPPLY => "No supply",
        _ => "Unknown",
    }
}

fn is_status_ok(s: u8) -> bool {
    s <= STATUS_RUNNING_CLOSED_LOOP
}

/// rusEFI wideband standard frame: version 0xA0, lambda ×10000 LE, temp °C LE.
/// Accepts A0 00 00 00 00 00 00 00 (warmup/invalid) and valid ranges.
fn looks_like_standard_frame(data: &[u8]) -> bool {
    if data.len() != 8 {
        return false;
    }
    if data[0] != 0xA0 {
        return false;
    }
    let lambda_raw = u16::from_le_bytes([data[2], data[3]]);
    let lambda = lambda_raw as f32 / 10000.0;
    let temp_raw = i16::from_le_bytes([data[4], data[5]]);
    (0.0..=2.0).contains(&lambda) && (-40..=1000).contains(&temp_raw)
}

/// rusEFI wideband diagnostic frame: ESR Ω, Nernst ×1000 V, status 0–6.
/// Valid ranges: ESR 0–1000, Nernst 0–2000 (0–2V), status 0–6.
fn looks_like_diagnostic_frame(data: &[u8]) -> bool {
    if data.len() != 8 {
        return false;
    }
    let esr = u16::from_le_bytes([data[0], data[1]]);
    let nernst_raw = u16::from_le_bytes([data[2], data[3]]);
    let nernst_v = nernst_raw as f32 / 1000.0;
    let status = data[5];
    esr <= 1000 && nernst_v <= 2.0 && status <= 6
}

/// Pong response data (from WB_ACK_ID with DLC=8)
/// Layout: baseId (low, high), Version (0xA0), year, month, day, reserved
#[derive(Clone, Debug, Default)]
struct PongData {
    base_id: u16,
    version: u8,
    year: u8,
    month: u8,
    day: u8,
}

/// AEMNet UEGO data (0x180+offset, big-endian)
#[derive(Clone, Debug, Default)]
struct AemUegoData {
    index: u8,
    lambda_value: f32,
    afr_gasoline: f32,
    valid: bool,
    system_volts: f32,
    lsu_49: bool,
    faults: u8,
}

/// AEMNet EGT data (0x0A0305+offset, big-endian)
#[derive(Clone, Debug, Default)]
struct AemEgtData {
    index: u8,
    temp_c: f32,
}

/// Standard sensor data (lambda, temperature)
#[derive(Clone, Debug, Default)]
struct WidebandData {
    index: u8,
    valid: bool,
    lambda_value: f32,
    afr_gasoline: f32,
    temp_c: i16,
}

/// Diagnostic data (ESR, Nernst, status)
#[derive(Clone, Debug, Default)]
struct WidebandDiagnostics {
    index: u8,
    esr_ohms: u16,
    nernst_v: f32,
    pump_duty_pct: f32,
    heater_duty_pct: f32,
    status: u8,
}

/// Key: (base_id, channel_index)
type SensorKey = (u16, u8);

pub struct RusefiWidebandPlugin {
    sensor_data: HashMap<SensorKey, WidebandData>,
    diagnostics: HashMap<SensorKey, WidebandDiagnostics>,
    /// Base IDs discovered by validating decoded values against rusEFI structure
    discovered_bases: HashSet<u16>,
    /// AEMNet UEGO data (when AemNetTx enabled)
    aem_uego_data: HashMap<u8, AemUegoData>,
    /// AEMNet EGT data (when egt[ch].AemNetTx enabled)
    aem_egt_data: HashMap<u8, AemEgtData>,
    /// Pong response (FW version, build date)
    pong: Option<PongData>,
    /// Heater enabled (sends ECU status periodically)
    heater_enabled: bool,
    /// Battery voltage for ECU status (×10, e.g. 135 = 13.5V)
    voltage_x10: u8,
    /// Optional pump gain for ECU status (0–200 = 0–200%, DLC≥3)
    pump_gain_x100: Option<u8>,
    /// Bus to use for TX
    tx_bus: u8,
    /// Last ECU status send time
    last_ecu_status: Option<Instant>,
    /// ECU status interval (10ms per protocol)
    ecu_status_interval: Duration,
    /// Restart: stop heater for 0.5s (10 × 50ms)
    restart_remaining: u8,
    /// Target device HW ID (0x000-0x0FF hex address; 0xFF = broadcast)
    hw_index: u8,
    /// RusEfi base CAN ID (11-bit, 0–0x7FF) for SetIndex and decode. StandardData at base, DiagData at base+1.
    rusefi_base_id: u16,
    /// Sensor type for Set Sensor Type (0-3)
    sensor_type: u8,
    /// Command status
    cmd_status: CmdStatus,
    /// When we last sent a command (for timeout)
    cmd_sent_at: Option<Instant>,
    /// Combined edit buffer for "base:target" (e.g. 190:0FF)
    id_edit_buf: String,
}

impl RusefiWidebandPlugin {
    pub fn new() -> Self {
        Self {
            sensor_data: HashMap::new(),
            diagnostics: HashMap::new(),
            discovered_bases: HashSet::new(),
            aem_uego_data: HashMap::new(),
            aem_egt_data: HashMap::new(),
            pong: None,
            heater_enabled: false,
            voltage_x10: 135, // 13.5V
            pump_gain_x100: None,
            tx_bus: 0,
            last_ecu_status: None,
            ecu_status_interval: Duration::from_millis(10),
            restart_remaining: 0,
            hw_index: 0xff, // Broadcast by default
            rusefi_base_id: 0x190,
            sensor_type: 0, // LSU 4.9
            cmd_status: CmdStatus::Idle,
            cmd_sent_at: None,
            id_edit_buf: "190".to_string(),
        }
    }

    fn decode_standard_frame(&self, can_id: u32, data: &[u8], base: u32) -> Option<WidebandData> {
        if data.len() != 8 {
            return None;
        }
        let offset = can_id - base;
        if offset > 30 || offset % 2 != 0 {
            return None;
        }

        let index = (offset / 2) as u8;
        let valid = data[1] == 1;
        let lambda_raw = u16::from_le_bytes([data[2], data[3]]);
        let temp_raw = i16::from_le_bytes([data[4], data[5]]);

        let lambda_value = lambda_raw as f32 / 10000.0;
        let afr = lambda_value * 14.7;

        Some(WidebandData {
            index,
            valid,
            lambda_value,
            afr_gasoline: afr,
            temp_c: temp_raw,
        })
    }

    fn decode_diagnostic_frame(&self, can_id: u32, data: &[u8], base: u32) -> Option<WidebandDiagnostics> {
        if data.len() != 8 {
            return None;
        }
        let offset = can_id - base;
        if offset == 0 || offset > 31 || offset % 2 != 1 {
            return None;
        }

        let index = ((offset - 1) / 2) as u8;
        let esr = u16::from_le_bytes([data[0], data[1]]);
        let nernst_mv = u16::from_le_bytes([data[2], data[3]]) as f32 / 1000.0;
        let pump_duty = data[4] as f32 / 2.55;
        let status = data[5];
        let heater_duty = data[6] as f32 / 2.55;

        Some(WidebandDiagnostics {
            index,
            esr_ohms: esr,
            nernst_v: nernst_mv,
            pump_duty_pct: pump_duty,
            heater_duty_pct: heater_duty,
            status,
        })
    }

    fn decode_pong(&self, data: &[u8]) -> Option<PongData> {
        if data.len() != 8 {
            return None;
        }
        // New layout: baseId (low, high), Version (0xA0), year, month, day, reserved
        if data[2] == 0xA0 {
            Some(PongData {
                base_id: u16::from_le_bytes([data[0], data[1]]),
                version: data[2],
                year: data[3],
                month: data[4],
                day: data[5],
            })
        } else if data[1] == 0xA0 {
            // Legacy: hwId, Version, year, month, day
            Some(PongData {
                base_id: data[0] as u16,
                version: data[1],
                year: data[2],
                month: data[3],
                day: data[4],
            })
        } else {
            // Older legacy: version, day, month, year
            Some(PongData {
                base_id: 0,
                version: data[0],
                year: data[3],
                month: data[2],
                day: data[1],
            })
        }
    }

    /// Decode AEMNet UEGO (0x180+offset, big-endian)
    fn decode_aem_uego(&self, can_id: u32, data: &[u8]) -> Option<AemUegoData> {
        if data.len() != 8 {
            return None;
        }
        if can_id < AEM_UEGO_BASE || can_id > AEM_UEGO_BASE + 15 {
            return None;
        }
        let index = (can_id - AEM_UEGO_BASE) as u8;
        let lambda_raw = u16::from_be_bytes([data[0], data[1]]);
        let lambda_value = lambda_raw as f32 / 10000.0;
        let system_volts = data[4] as f32 / 10.0;
        let flags = data[6];
        let valid = (flags & 0x80) != 0;
        let lsu_49 = (flags & 0x02) != 0;
        Some(AemUegoData {
            index,
            lambda_value,
            afr_gasoline: lambda_value * 14.7,
            valid,
            system_volts,
            lsu_49,
            faults: data[7],
        })
    }

    /// Decode AEMNet EGT (0x0A0305+offset, big-endian)
    /// EGT typically: bytes 0-1 = temp × 10 (°C)
    fn decode_aem_egt(&self, can_id: u32, data: &[u8]) -> Option<AemEgtData> {
        if data.len() != 8 {
            return None;
        }
        if can_id < AEM_EGT_BASE || can_id > AEM_EGT_BASE + 15 {
            return None;
        }
        let index = (can_id - AEM_EGT_BASE) as u8;
        let temp_raw = u16::from_be_bytes([data[0], data[1]]);
        let temp_c = temp_raw as f32 / 10.0;
        Some(AemEgtData {
            index,
            temp_c,
        })
    }

    fn process_message(&mut self, msg: &crate::hardware::can_manager::ManagerMessage) {
        let can_id = msg.message.id;
        let data = &msg.message.data;

        // Auto-discover: validate against hardcoded rusEFI structure (base + base+1 pairing)
        if can_id <= 0x7FF && data.len() == 8 {
            if looks_like_standard_frame(data) {
                self.discovered_bases.insert(can_id as u16);
            } else if looks_like_diagnostic_frame(data) && can_id > 0 {
                self.discovered_bases.insert((can_id - 1) as u16);
            }
        }

        // Decode: user base + discovered bases (standard at base, diagnostic at base+1)
        let bases: Vec<u32> = std::iter::once(self.rusefi_base_id as u32)
            .chain(self.discovered_bases.iter().copied().map(|b| b as u32))
            .collect();

        for base in bases {
            if (base..=base + 31).contains(&can_id) {
                let offset = can_id - base;
                if offset % 2 == 0 {
                    if let Some(d) = self.decode_standard_frame(can_id, data, base) {
                        self.sensor_data.insert((base as u16, d.index), d);
                    }
                } else if let Some(d) = self.decode_diagnostic_frame(can_id, data, base) {
                    self.diagnostics.insert((base as u16, d.index), d);
                }
                break;
            }
        }

        if (AEM_UEGO_BASE..=AEM_UEGO_BASE + 15).contains(&can_id) {
            if let Some(d) = self.decode_aem_uego(can_id, data) {
                self.aem_uego_data.insert(d.index, d);
            }
        } else if (AEM_EGT_BASE..=AEM_EGT_BASE + 15).contains(&can_id) {
            if let Some(d) = self.decode_aem_egt(can_id, data) {
                self.aem_egt_data.insert(d.index, d);
            }
        } else if can_id == WB_ACK_ID {
            debug!("Wideband: received WB_ACK_ID 0x{:X} DLC={}", can_id, data.len());
            if data.len() == 8 {
                if let Some(pong) = self.decode_pong(data) {
                    let base_id = pong.base_id;
                    self.pong = Some(pong);
                    self.rusefi_base_id = base_id;
                    self.id_edit_buf = format!("{:03X}", base_id);
                }
            }
            // ACK (DLC=0) or Pong (DLC=8) - both indicate command completed
            self.cmd_status = CmdStatus::Done;
            self.cmd_sent_at = None;
        }
    }

    /// Build ECU status message for heater enable (WidebandControl 0xEF50000)
    /// DLC 2: BatteryVoltage, HeaterEnable
    /// DLC ≥3: + optional PumpGain (0–200 = 0–200%)
    fn build_ecu_status(&self) -> CanMessage {
        let flags = if self.heater_enabled && self.restart_remaining == 0 {
            0x01u8
        } else {
            0x00
        };
        let mut data = vec![self.voltage_x10, flags];
        if let Some(gain) = self.pump_gain_x100 {
            data.push(gain);
        }
        CanMessage::new(self.tx_bus, WB_CMD_ECU_STATUS, data.into())
    }

    /// Ping (0xEF60000): payload = base CAN ID as [high, low] (e.g. 190 → 01 90)
    fn queue_ping(&self, ctx: &mut crate::plugins::PluginContext) {
        let data = vec![
            ((self.rusefi_base_id >> 8) & 0xFF) as u8,
            (self.rusefi_base_id & 0xFF) as u8,
        ];
        let msg = CanMessage::new(self.tx_bus, WB_CMD_PING, data.into());
        for _ in 0..100 {
            ctx.queue_send.push((self.tx_bus, msg.clone()));
        }
    }

    /// SetIndex (0xEF40000): payload = base ID as [high, low] (e.g. 190 → 01 90).
    /// DLC 2 = broadcast. DLC 3 when hw_index == 0 (target device 0, byte 2 = 0).
    fn queue_set_index(&self, ctx: &mut crate::plugins::PluginContext) {
        let base = self.rusefi_base_id as u32;
        let data: Vec<u8> = if self.hw_index == 0 {
            vec![((base >> 8) & 0xFF) as u8, (base & 0xFF) as u8, 0]
        } else {
            vec![((base >> 8) & 0xFF) as u8, (base & 0xFF) as u8]
        };
        let msg = CanMessage::new(self.tx_bus, WB_CMD_SET_INDEX, data.into());
        for _ in 0..100 {
            ctx.queue_send.push((self.tx_bus, msg.clone()));
        }
    }

    fn queue_set_sensor_type(&self, ctx: &mut crate::plugins::PluginContext) {
        let data = vec![self.hw_index, self.sensor_type];
        ctx.queue_send.push((
            self.tx_bus,
            CanMessage::new(self.tx_bus, WB_CMD_SET_SENSOR_TYPE, data.into()),
        ));
    }
}

impl Plugin for RusefiWidebandPlugin {
    fn id(&self) -> &str {
        "rusefi_wideband"
    }

    fn name(&self) -> &str {
        "rusEFI Wideband"
    }

    fn description(&self) -> &str {
        "rusEFI Wideband Tools - lambda/AFR, Ping, Set Index, Set Sensor Type, Restart"
    }

    fn render(
        &mut self,
        ui: &Ui,
        ctx: &mut crate::plugins::PluginContext,
        messages: &[crate::hardware::can_manager::ManagerMessage],
        is_open: &mut bool,
    ) {
        // Process incoming messages
        for msg in messages {
            self.process_message(msg);
        }

        // Restart countdown
        if self.restart_remaining > 0 {
            self.restart_remaining = self.restart_remaining.saturating_sub(1);
        }

        // Timeout Busy after 500ms with no response
        if self.cmd_status == CmdStatus::Busy {
            if let Some(sent_at) = self.cmd_sent_at {
                if sent_at.elapsed() > Duration::from_millis(500) {
                    self.cmd_status = CmdStatus::Failed;
                    self.cmd_sent_at = None;
                }
            }
        }

        // Queue ECU status periodically when heater enabled (and not in restart)
        if ctx.is_connected && self.heater_enabled {
            let now = Instant::now();
            let should_send = self
                .last_ecu_status
                .map_or(true, |t| now.duration_since(t) >= self.ecu_status_interval);
            if should_send {
                let ecu_msg = self.build_ecu_status();
                ctx.queue_send.push((self.tx_bus, ecu_msg));
                self.last_ecu_status = Some(now);
            }
        }

        ui.window("rusEFI Wideband Tools")
            .size([420.0, 580.0], Condition::FirstUseEver)
            .position([50.0, 100.0], Condition::FirstUseEver)
            .opened(is_open)
            .build(|| {
                if !ctx.is_connected && !ctx.has_playback {
                    ui.text_colored([1.0, 0.5, 0.3, 1.0], "No CAN interface connected");
                    ui.text_wrapped("Connect to a CAN interface in Hardware Manager first, or open a CAN log (CSV/rlog) for playback. The wideband controller requires ECU status messages every 10ms to enable the heater when live.");
                    return;
                }
                if ctx.has_playback && !ctx.is_connected {
                    ui.text_colored([0.5, 0.8, 0.5, 1.0], "Playback mode");
                    ui.text_wrapped("Showing wideband data from loaded log. Heater and commands require a live CAN connection.");
                    ui.separator();
                }

                if ctx.is_connected {
                ui.text("Configuration");
                ui.separator();

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
                    let labels_ref: Vec<&str> = bus_labels.iter().map(|s| s.as_str()).collect();
                    if ui.combo_simple_string("##tx_bus", &mut bus_idx, &labels_ref) {
                        if let Some(&bus) = ctx.connected_buses.get(bus_idx) {
                            self.tx_bus = bus;
                        }
                    }
                } else {
                    ui.text("(no buses)");
                }

                ui.text("Battery voltage:");
                ui.same_line();
                let mut v = self.voltage_x10 as i32;
                if ui.input_int("##voltage", &mut v).build() {
                    self.voltage_x10 = v.clamp(80, 160) as u8; // 8.0V - 16.0V
                }
                ui.same_line();
                ui.text(format!("({:.1}V)", self.voltage_x10 as f32 / 10.0));

                ui.checkbox("Heater enabled", &mut self.heater_enabled);
                ui.same_line();
                ui.text_disabled("(?)");
                if ui.is_item_hovered() {
                    ui.tooltip(|| {
                        let _wrap = ui.push_text_wrap_pos_with_pos(350.0);
                        ui.text_wrapped("The controller requires ECU status with heater bit set to produce valid data. Warm-up takes 10-30 seconds.");
                    });
                }

                ui.text("Pump gain (optional, DLC≥3):");
                ui.same_line();
                let mut pump_str = self
                    .pump_gain_x100
                    .map(|g| g.to_string())
                    .unwrap_or_else(String::new);
                if ui.input_text("##pump_gain", &mut pump_str)
                    .hint("100 = 100%")
                    .build()
                {
                    if pump_str.trim().is_empty() {
                        self.pump_gain_x100 = None;
                    } else if let Ok(g) = pump_str.trim().parse::<u8>() {
                        if g <= 200 {
                            self.pump_gain_x100 = Some(g);
                        }
                    }
                }
                ui.same_line();
                ui.text_disabled("(?)");
                if ui.is_item_hovered() {
                    ui.tooltip(|| {
                        let _wrap = ui.push_text_wrap_pos_with_pos(350.0);
                        ui.text_wrapped("Optional pump controller gain 0-200% sent in WidebandControl byte 2. Leave empty to send DLC 2 only.");
                    });
                }

                ui.separator();
                ui.text("2025 firmware tools");
                ui.separator();

                // Single value used for both base ID and target HW ID
                ui.text("ID");
                ui.same_line();
                ui.text_disabled("(?)");
                if ui.is_item_hovered() {
                    ui.tooltip(|| {
                        let _wrap = ui.push_text_wrap_pos_with_pos(350.0);
                        ui.text_wrapped("Base CAN ID for Ping/SetIndex. Auto-detects sensors by validating decoded values against rusEFI structure (lambda 0.5–1.5, temp -40–1000°C, ESR 0–1000Ω, etc.).");
                    });
                }
                ui.same_line();
                if ui.input_text("##id", &mut self.id_edit_buf)
                    .hint("190")
                    .build()
                {
                    let s = self.id_edit_buf.trim().trim_start_matches("0x");
                    if let Ok(n) = u16::from_str_radix(s, 16) {
                        self.rusefi_base_id = n.min(0x7FF);
                        self.hw_index = (n.min(0xFF)) as u8;
                    }
                    self.id_edit_buf = format!("{:03X}", self.rusefi_base_id);
                }
                ui.same_line();
                if ui.button("Ping") {
                    self.cmd_status = CmdStatus::Busy;
                    self.cmd_sent_at = Some(Instant::now());
                    self.pong = None;
                    self.queue_ping(ctx);
                }
                ui.same_line();
                if ui.button("Set Index") {
                    self.cmd_status = CmdStatus::Busy;
                    self.cmd_sent_at = Some(Instant::now());
                    self.queue_set_index(ctx);
                }
                ui.same_line();
                let status_str = match self.cmd_status {
                    CmdStatus::Idle => "Idle",
                    CmdStatus::Done => "Done",
                    CmdStatus::Busy => "Busy",
                    CmdStatus::Failed => "Failed",
                };
                let status_color = match self.cmd_status {
                    CmdStatus::Idle => [0.6, 0.6, 0.6, 1.0],
                    CmdStatus::Done => [0.3, 0.8, 0.3, 1.0],
                    CmdStatus::Busy => [0.8, 0.8, 0.3, 1.0],
                    CmdStatus::Failed => [1.0, 0.3, 0.3, 1.0],
                };
                ui.text_colored(status_color, status_str);

                // FW version and build date
                if let Some(ref p) = self.pong {
                    let year_display = 2000u32 + p.year as u32;
                    ui.text(format!(
                        "Base 0x{:03X}  FW: v{}  Build: {}/{}/{}",
                        p.base_id, p.version, p.day, p.month, year_display
                    ));
                } else {
                    ui.text_colored([0.5, 0.5, 0.5, 1.0], "FW version: (ping to get)");
                }
                ui.same_line();
                ui.text_disabled("(?)");
                if ui.is_item_hovered() {
                    ui.tooltip(|| {
                        let _wrap = ui.push_text_wrap_pos_with_pos(350.0);
                        ui.text_wrapped("Ping sends ID as [high, low] over 0xEF60000 (e.g. 190 → 01 90). Controller replies with Pong if it matches.");
                    });
                }

                ui.separator();

                // Sensor type (LsuSensorType in TunerStudio)
                ui.text("Sensor type:");
                ui.same_line();
                ui.text_disabled("(?)");
                if ui.is_item_hovered() {
                    ui.tooltip(|| {
                        let _wrap = ui.push_text_wrap_pos_with_pos(350.0);
                        ui.text_wrapped("LSU 4.9, LSU 4.2, LSU ADV, or FAE LSU 4.9. Must match sensor hardware.");
                    });
                }
                ui.same_line();
                let mut sens_idx = self.sensor_type as usize;
                if ui.combo_simple_string("##sensor_type", &mut sens_idx, &SENSOR_NAMES) {
                    self.sensor_type = sens_idx as u8;
                }

                if ui.button("Set sensor type") {
                    self.cmd_status = CmdStatus::Busy;
                    self.cmd_sent_at = Some(Instant::now());
                    self.queue_set_sensor_type(ctx);
                }

                ui.separator();

                // Restart
                if ui.button("Restart all WBO") {
                    self.restart_remaining = 50; // 0.5s at 10ms interval (matches firmware)
                }
                ui.same_line();
                ui.text_disabled("(?)");
                if ui.is_item_hovered() {
                    ui.tooltip(|| {
                        let _wrap = ui.push_text_wrap_pos_with_pos(350.0);
                        ui.text_wrapped("Stops sending heater enable for ~0.5s. Use to reset controllers.");
                    });
                }

                ui.separator();
                } // end if ctx.is_connected

                ui.text("Sensor Data");
                ui.same_line();
                ui.text_disabled("(?)");
                if ui.is_item_hovered() {
                    ui.tooltip(|| {
                        let _wrap = ui.push_text_wrap_pos_with_pos(350.0);
                        ui.text_wrapped("Typical ranges: λ 0.5-1.3, AFR 6.5-20, Temp 500-1050°C, ESR 200-400Ω, Nernst 0-0.9V. RusEfi 100 Hz, AEMNet UEGO 100 Hz, AEMNet EGT 20 Hz. Status: Preheat→Warmup→Running = OK.");
                    });
                }
                ui.separator();

                let mut keys: Vec<SensorKey> = self.sensor_data.keys().copied().collect();
                keys.sort_by_key(|(base, idx)| (*base, *idx));

                if keys.is_empty() && self.aem_uego_data.is_empty() && self.aem_egt_data.is_empty() {
                    ui.text_colored([0.6, 0.6, 0.6, 1.0], "No wideband data received yet");
                    ui.text_wrapped(format!(
                        "Ensure heater is enabled and wait 10-30s for warm-up. Auto-detects by structure. Configured: 0x{:03X}. AEMNet UEGO: 0x180+. AEMNet EGT: 0x0A0305+.",
                        self.rusefi_base_id
                    ));
                } else {
                    for (base, idx) in keys {
                        let key = (base, idx);
                        let data = self.sensor_data.get(&key).unwrap();
                        let diag = self.diagnostics.get(&key);

                        let valid_color = if data.valid {
                            [0.3, 0.8, 0.3, 1.0]
                        } else {
                            [0.6, 0.5, 0.3, 1.0]
                        };
                        ui.text_colored(
                            valid_color,
                            format!("0x{:03X} Ch{}", base, idx),
                        );

                        ui.indent();
                        if data.valid {
                            ui.text(format!("  λ: {:.4}", data.lambda_value));
                            ui.text(format!("  AFR: {:.2}", data.afr_gasoline));
                        } else {
                            ui.text_colored(
                                [0.6, 0.6, 0.6, 1.0],
                                "  (warming up or invalid)",
                            );
                        }
                        ui.text(format!("  Temp: {} °C", data.temp_c));

                        if let Some(d) = diag {
                            let status_color = if is_status_ok(d.status) {
                                [0.3, 0.8, 0.3, 1.0] // green for Preheat/Warmup/Running
                            } else {
                                [1.0, 0.4, 0.2, 1.0] // orange-red for faults
                            };
                            ui.text_colored(
                                status_color,
                                format!("  Status: {}", status_name(d.status)),
                            );
                            ui.text(format!(
                                "  ESR: {} Ω  Nernst: {:.3}V",
                                d.esr_ohms, d.nernst_v
                            ));
                            ui.text(format!(
                                "  Pump: {:.0}%  Heater: {:.0}%",
                                d.pump_duty_pct, d.heater_duty_pct
                            ));
                        }
                        ui.unindent();
                    }
                }

                if !self.aem_uego_data.is_empty() {
                    ui.separator();
                    ui.text("AEMNet UEGO");
                    ui.separator();
                    let mut aem_idx: Vec<u8> = self.aem_uego_data.keys().copied().collect();
                    aem_idx.sort();
                    for idx in aem_idx {
                        let d = self.aem_uego_data.get(&idx).unwrap();
                        let valid_color = if d.valid {
                            [0.3, 0.8, 0.3, 1.0]
                        } else {
                            [0.6, 0.5, 0.3, 1.0]
                        };
                        ui.text_colored(valid_color, format!("UEGO {}", idx));
                        ui.indent();
                        ui.text(format!("  λ: {:.4}  AFR: {:.2}", d.lambda_value, d.afr_gasoline));
                        ui.text(format!("  System: {:.1}V  LSU4.9: {}", d.system_volts, d.lsu_49));
                        if d.faults != 0 {
                            ui.text_colored([1.0, 0.4, 0.2, 1.0], format!("  Faults: {}", d.faults));
                        }
                        ui.unindent();
                    }
                }

                if !self.aem_egt_data.is_empty() {
                    ui.separator();
                    ui.text("AEMNet EGT");
                    ui.separator();
                    let mut egt_idx: Vec<u8> = self.aem_egt_data.keys().copied().collect();
                    egt_idx.sort();
                    for idx in egt_idx {
                        let d = self.aem_egt_data.get(&idx).unwrap();
                        ui.text(format!("EGT {}: {:.1} °C", idx, d.temp_c));
                    }
                }

                ui.separator();
                ui.text_disabled("Protocol: RusEfi 0x190+, AEMNet UEGO 0x180+, EGT 0x0A0305+, 500kbps");
            });
    }
}
