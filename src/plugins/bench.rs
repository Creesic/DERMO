//! rusEFI Bench Test CAN Plugin
//!
//! Implements the rusEFI Bench Test protocol (base 0x770000).
//! ECU transmits bench data; STIM sends control commands.
//! See rusefi-hardware digital-inputs firmware and bench DBC.
//!
//! Protocol: Extended IDs 0x770000-0x770015, 8-byte frames.
//! Raw ADC: 0-255 -> 0-5V (factor 0.0196078).

use crate::core::CanMessage;
use crate::plugins::Plugin;
use imgui::{Condition, Ui};
use std::time::Instant;

const BENCH_BASE: u32 = 0x770000;
const BENCH_HEADER: u8 = 0x66;

// Raw ADC: 0-255 -> 0-5V
const RAW_ADC_FACTOR: f32 = 0.0196078;

fn get_u8(data: &[u8], i: usize) -> u8 {
    data.get(i).copied().unwrap_or(0)
}

fn raw_to_volts(raw: u8) -> f32 {
    raw as f32 * RAW_ADC_FACTOR
}

/// Bench_EventCounters (0x770000)
#[derive(Clone, Debug, Default)]
struct BenchEventCounters {
    primary_trigger: u8,
    secondary_trigger: u8,
    vvt_cam: [u8; 4],
    vehicle_speed: u8,
}

/// Bench_RawAnalog1 (0x770001)
#[derive(Clone, Debug, Default)]
struct BenchRawAnalog1 {
    tps1_primary: f32,
    tps1_secondary: f32,
    accelerator_primary: f32,
    accelerator_secondary: f32,
    map_slow: f32,
    clt: f32,
    iat: f32,
    battery: f32,
}

/// Bench_BoardStatus (0x770003)
#[derive(Clone, Debug, Default)]
struct BenchBoardStatus {
    board_id: u16,
    seconds_since_reset: u32,
    engine_type: u16,
}

/// Bench_ButtonCounters (0x770004)
#[derive(Clone, Debug, Default)]
struct BenchButtonCounters {
    brake_pedal: u8,
    clutch_up: u8,
    ac_button: u8,
}

/// Bench_IoMetaInfo (0x770005)
#[derive(Clone, Debug, Default)]
struct BenchIoMetaInfo {
    outputs_count: u8,
    low_side_count: u8,
    dc_outputs_count: u8,
}

/// Bench_RawAnalog2 (0x770006)
#[derive(Clone, Debug, Default)]
struct BenchRawAnalog2 {
    tps2_primary: f32,
    tps2_secondary: f32,
    aux_linear1: f32,
    aux_linear2: f32,
    oil_pressure: f32,
    fuel_pressure_low: f32,
    fuel_pressure_high: f32,
    aux_temp1: f32,
}

/// Bench_PinState (0x770007)
#[derive(Clone, Debug, Default)]
struct BenchPinState {
    pin_toggle_counter: u16,
    duration_state0_ms: u32,
    duration_state1_ms: u32,
}

/// Bench_AuxDigitalCounters (0x770008)
#[derive(Clone, Debug, Default)]
struct BenchAuxDigitalCounters {
    lua_digital: [u8; 8],
}

/// Bench_RawLuaAnalog1 (0x770013)
#[derive(Clone, Debug, Default)]
struct BenchRawLuaAnalog1 {
    aux_analog: [f32; 8],
}

/// Bench_EcuGetCalibration (0x770010)
#[derive(Clone, Debug, Default)]
struct BenchEcuGetCalibration {
    hash: u32,
    value: f32,
}

pub struct RusefiBenchPlugin {
    /// Received data by message ID
    event_counters: Option<BenchEventCounters>,
    raw_analog1: Option<BenchRawAnalog1>,
    board_status: Option<BenchBoardStatus>,
    button_counters: Option<BenchButtonCounters>,
    io_meta: Option<BenchIoMetaInfo>,
    raw_analog2: Option<BenchRawAnalog2>,
    pin_state: Option<BenchPinState>,
    aux_digital: Option<BenchAuxDigitalCounters>,
    raw_lua_analog1: Option<BenchRawLuaAnalog1>,
    ecu_calibration: Option<BenchEcuGetCalibration>,
    last_update: Instant,
    tx_bus: u8,
    /// IoControl: command (0-8), data bytes
    cmd_io_control: u8,
    cmd_data2: u8,
    cmd_data3: u8,
    /// UserCtrl: subsystem (LE16), index (LE16)
    user_ctrl_subsystem: u16,
    user_ctrl_index: u16,
    /// ReqCal: field hash (LE32)
    req_cal_hash: u32,
    last_send: Option<Instant>,
}

impl RusefiBenchPlugin {
    pub fn new() -> Self {
        Self {
            event_counters: None,
            raw_analog1: None,
            board_status: None,
            button_counters: None,
            io_meta: None,
            raw_analog2: None,
            pin_state: None,
            aux_digital: None,
            raw_lua_analog1: None,
            ecu_calibration: None,
            last_update: Instant::now(),
            tx_bus: 0,
            cmd_io_control: 0,
            cmd_data2: 0,
            cmd_data3: 0,
            user_ctrl_subsystem: 0x0014, // example: start/stop engine
            user_ctrl_index: 0x0009,
            req_cal_hash: 0,
            last_send: None,
        }
    }

    fn process_message(&mut self, msg: &crate::hardware::can_manager::ManagerMessage) {
        let id = msg.message.id;
        let data = &msg.message.data;

        if id < BENCH_BASE || id > BENCH_BASE + 0x20 {
            return;
        }

        let sub = id - BENCH_BASE;
        self.last_update = Instant::now();

        match sub {
            0 => {
                if data.len() >= 8 {
                    self.event_counters = Some(BenchEventCounters {
                        primary_trigger: get_u8(data, 0),
                        secondary_trigger: get_u8(data, 1),
                        vvt_cam: [
                            get_u8(data, 2),
                            get_u8(data, 3),
                            get_u8(data, 4),
                            get_u8(data, 5),
                        ],
                        vehicle_speed: get_u8(data, 6),
                        ..Default::default()
                    });
                }
            }
            1 => {
                if data.len() >= 8 {
                    self.raw_analog1 = Some(BenchRawAnalog1 {
                        tps1_primary: raw_to_volts(get_u8(data, 0)),
                        tps1_secondary: raw_to_volts(get_u8(data, 1)),
                        accelerator_primary: raw_to_volts(get_u8(data, 2)),
                        accelerator_secondary: raw_to_volts(get_u8(data, 3)),
                        map_slow: raw_to_volts(get_u8(data, 4)),
                        clt: raw_to_volts(get_u8(data, 5)),
                        iat: raw_to_volts(get_u8(data, 6)),
                        battery: raw_to_volts(get_u8(data, 7)),
                    });
                }
            }
            3 => {
                if data.len() >= 8 {
                    let sec_hi = (get_u8(data, 2) as u32) << 16;
                    let sec_mid = (get_u8(data, 3) as u32) << 8;
                    let sec_lo = get_u8(data, 4) as u32;
                    self.board_status = Some(BenchBoardStatus {
                        board_id: (get_u8(data, 0) as u16) << 8 | get_u8(data, 1) as u16,
                        seconds_since_reset: sec_hi | sec_mid | sec_lo,
                        engine_type: (get_u8(data, 5) as u16) << 8 | get_u8(data, 6) as u16,
                        ..Default::default()
                    });
                }
            }
            4 => {
                if data.len() >= 3 {
                    self.button_counters = Some(BenchButtonCounters {
                        brake_pedal: get_u8(data, 0),
                        clutch_up: get_u8(data, 1),
                        ac_button: get_u8(data, 2),
                        ..Default::default()
                    });
                }
            }
            5 => {
                if data.len() >= 5 {
                    self.io_meta = Some(BenchIoMetaInfo {
                        outputs_count: get_u8(data, 2),
                        low_side_count: get_u8(data, 3),
                        dc_outputs_count: get_u8(data, 4),
                        ..Default::default()
                    });
                }
            }
            6 => {
                if data.len() >= 8 {
                    self.raw_analog2 = Some(BenchRawAnalog2 {
                        tps2_primary: raw_to_volts(get_u8(data, 0)),
                        tps2_secondary: raw_to_volts(get_u8(data, 1)),
                        aux_linear1: raw_to_volts(get_u8(data, 2)),
                        aux_linear2: raw_to_volts(get_u8(data, 3)),
                        oil_pressure: raw_to_volts(get_u8(data, 4)),
                        fuel_pressure_low: raw_to_volts(get_u8(data, 5)),
                        fuel_pressure_high: raw_to_volts(get_u8(data, 6)),
                        aux_temp1: raw_to_volts(get_u8(data, 7)),
                    });
                }
            }
            7 => {
                if data.len() >= 8 {
                    let d0_hi = (get_u8(data, 2) as u32) << 16;
                    let d0_mid = (get_u8(data, 3) as u32) << 8;
                    let d0_lo = get_u8(data, 4) as u32;
                    let d1_hi = (get_u8(data, 5) as u32) << 16;
                    let d1_mid = (get_u8(data, 6) as u32) << 8;
                    let d1_lo = get_u8(data, 7) as u32;
                    self.pin_state = Some(BenchPinState {
                        pin_toggle_counter: (get_u8(data, 0) as u16) << 8 | get_u8(data, 1) as u16,
                        duration_state0_ms: d0_hi | d0_mid | d0_lo,
                        duration_state1_ms: d1_hi | d1_mid | d1_lo,
                        ..Default::default()
                    });
                }
            }
            8 => {
                if data.len() >= 8 {
                    self.aux_digital = Some(BenchAuxDigitalCounters {
                        lua_digital: [
                            get_u8(data, 0),
                            get_u8(data, 1),
                            get_u8(data, 2),
                            get_u8(data, 3),
                            get_u8(data, 4),
                            get_u8(data, 5),
                            get_u8(data, 6),
                            get_u8(data, 7),
                        ],
                        ..Default::default()
                    });
                }
            }
            0x10 => {
                if data.len() >= 8 {
                    let hash = u32::from_le_bytes([
                        get_u8(data, 0),
                        get_u8(data, 1),
                        get_u8(data, 2),
                        get_u8(data, 3),
                    ]);
                    let value = f32::from_le_bytes([
                        get_u8(data, 4),
                        get_u8(data, 5),
                        get_u8(data, 6),
                        get_u8(data, 7),
                    ]);
                    self.ecu_calibration = Some(BenchEcuGetCalibration { hash, value });
                }
            }
            0x13 => {
                if data.len() >= 8 {
                    self.raw_lua_analog1 = Some(BenchRawLuaAnalog1 {
                        aux_analog: [
                            raw_to_volts(get_u8(data, 0)),
                            raw_to_volts(get_u8(data, 1)),
                            raw_to_volts(get_u8(data, 2)),
                            raw_to_volts(get_u8(data, 3)),
                            raw_to_volts(get_u8(data, 4)),
                            raw_to_volts(get_u8(data, 5)),
                            raw_to_volts(get_u8(data, 6)),
                            raw_to_volts(get_u8(data, 7)),
                        ],
                        ..Default::default()
                    });
                }
            }
            _ => {}
        }
    }

    fn queue_io_control(&self, ctx: &mut crate::plugins::PluginContext) {
        let data = vec![
            BENCH_HEADER,
            self.cmd_io_control,
            self.cmd_data2,
            self.cmd_data3,
            0,
            0,
            0,
            0,
        ];
        ctx.queue_send.push((self.tx_bus, CanMessage::new(self.tx_bus, BENCH_BASE + 2, data.into())));
    }

    fn queue_user_control(&self, ctx: &mut crate::plugins::PluginContext) {
        let data = vec![
            BENCH_HEADER,
            0,
            (self.user_ctrl_subsystem & 0xFF) as u8,
            (self.user_ctrl_subsystem >> 8) as u8,
            (self.user_ctrl_index & 0xFF) as u8,
            (self.user_ctrl_index >> 8) as u8,
            0,
            0,
        ];
        ctx.queue_send.push((self.tx_bus, CanMessage::new(self.tx_bus, BENCH_BASE + 0x0C, data.into())));
    }

    fn queue_req_calibration(&self, ctx: &mut crate::plugins::PluginContext) {
        let hash_bytes = self.req_cal_hash.to_le_bytes();
        let data = vec![
            BENCH_HEADER,
            0,
            hash_bytes[0],
            hash_bytes[1],
            hash_bytes[2],
            hash_bytes[3],
            0,
            0,
        ];
        ctx.queue_send.push((self.tx_bus, CanMessage::new(self.tx_bus, BENCH_BASE + 0x12, data.into())));
    }
}

const IO_CMD_NAMES: [&str; 9] = [
    "GET_COUNT",
    "OUTPUT_SET",
    "OUTPUT_CLEAR",
    "SET_ENGINE_TYPE",
    "EXECUTE_BENCH",
    "QUERY_PIN_STATE",
    "START_PIN_TEST",
    "END_PIN_TEST",
    "CAN_QC_ETB",
];

impl Plugin for RusefiBenchPlugin {
    fn id(&self) -> &str {
        "rusefi_bench"
    }

    fn name(&self) -> &str {
        "rusEFI Bench Test"
    }

    fn description(&self) -> &str {
        "rusEFI Bench Test CAN - ECU bench data, STIM control commands"
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

        ui.window("rusEFI Bench Test (0x770000)")
            .size([520.0, 620.0], Condition::FirstUseEver)
            .position([150.0, 140.0], Condition::FirstUseEver)
            .opened(is_open)
            .build(|| {
                if !ctx.is_connected && !ctx.has_playback {
                    ui.text_colored([1.0, 0.5, 0.3, 1.0], "No CAN interface connected");
                    ui.text_wrapped("Connect to CAN or open a log for playback. Sending commands requires a live connection.");
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
                        let labels_ref: Vec<&str> = bus_labels.iter().map(|s| s.as_str()).collect();
                        if ui.combo_simple_string("##tx_bus", &mut bus_idx, &labels_ref) {
                            if let Some(&bus) = ctx.connected_buses.get(bus_idx) {
                                self.tx_bus = bus;
                            }
                        }
                    }

                    ui.separator();
                    ui.text("Send commands (STIM -> ECU)");
                    ui.separator();

                    ui.text("IoControl (0x770002):");
                    ui.indent();
                    let mut cmd_idx = self.cmd_io_control as usize;
                    if cmd_idx >= IO_CMD_NAMES.len() {
                        cmd_idx = 0;
                    }
                    if ui.combo_simple_string("##cmd", &mut cmd_idx, &IO_CMD_NAMES) {
                        self.cmd_io_control = cmd_idx as u8;
                    }
                    let mut d2 = self.cmd_data2 as i32;
                    let mut d3 = self.cmd_data3 as i32;
                    ui.input_int("DataByte2", &mut d2).build();
                    ui.input_int("DataByte3", &mut d3).build();
                    self.cmd_data2 = d2.clamp(0, 255) as u8;
                    self.cmd_data3 = d3.clamp(0, 255) as u8;
                    if ui.button("Send IoControl") {
                        self.queue_io_control(ctx);
                        self.last_send = Some(Instant::now());
                    }
                    ui.unindent();

                    ui.text("UserControl (0x77000C):");
                    ui.indent();
                    let mut sub = self.user_ctrl_subsystem as i32;
                    let mut idx = self.user_ctrl_index as i32;
                    ui.input_int("Subsystem (LE16)", &mut sub).build();
                    ui.input_int("Index (LE16)", &mut idx).build();
                    self.user_ctrl_subsystem = sub.clamp(0, 65535) as u16;
                    self.user_ctrl_index = idx.clamp(0, 65535) as u16;
                    if ui.button("Send UserControl") {
                        self.queue_user_control(ctx);
                        self.last_send = Some(Instant::now());
                    }
                    ui.same_line();
                    ui.text_disabled("(?)");
                    if ui.is_item_hovered() {
                        ui.tooltip(|| {
                            ui.text_wrapped("Example: Subsystem=0x14, Index=0x09 = start/stop engine");
                        });
                    }
                    ui.unindent();

                    ui.text("ReqCalibration (0x770012):");
                    ui.indent();
                    let mut hash_hex = format!("{:08X}", self.req_cal_hash);
                    if ui.input_text("Field hash (hex)", &mut hash_hex).build() {
                        if let Ok(n) = u32::from_str_radix(hash_hex.trim().trim_start_matches("0x"), 16) {
                            self.req_cal_hash = n;
                        }
                    }
                    if ui.button("Request calibration") {
                        self.queue_req_calibration(ctx);
                        self.last_send = Some(Instant::now());
                    }
                    ui.unindent();

                    if let Some(t) = self.last_send {
                        if t.elapsed().as_secs() < 2 {
                            ui.text_colored([0.3, 0.8, 0.3, 1.0], "Command sent");
                        }
                    }

                    ui.separator();
                }

                ui.text("Received data (ECU -> STIM)");
                ui.separator();

                let has_data = self.event_counters.is_some()
                    || self.raw_analog1.is_some()
                    || self.board_status.is_some()
                    || self.button_counters.is_some();

                if !has_data {
                    ui.text_colored([0.6, 0.6, 0.6, 1.0], "No bench data received yet");
                    ui.text_wrapped("ECU sends 0x770000-0x770015 when enableExtendedCanBroadcast or QC mode.");
                } else {
                    if let Some(ref e) = self.event_counters {
                        ui.text("EventCounters (0x770000):");
                        ui.indent();
                        ui.text(format!(
                            "  Primary: {}  Secondary: {}  VSS: {}",
                            e.primary_trigger, e.secondary_trigger, e.vehicle_speed
                        ));
                        ui.text(format!("  VVT Cam: {:?}", e.vvt_cam));
                        ui.unindent();
                    }

                    if let Some(ref a) = self.raw_analog1 {
                        ui.text("RawAnalog1 (0x770001) [V]:");
                        ui.indent();
                        ui.text(format!(
                            "  TPS1: {:.2}/{:.2}  Pedal: {:.2}/{:.2}",
                            a.tps1_primary, a.tps1_secondary, a.accelerator_primary, a.accelerator_secondary
                        ));
                        ui.text(format!(
                            "  MAP: {:.2}  CLT: {:.2}  IAT: {:.2}  Batt: {:.2}",
                            a.map_slow, a.clt, a.iat, a.battery
                        ));
                        ui.unindent();
                    }

                    if let Some(ref b) = self.board_status {
                        ui.text("BoardStatus (0x770003):");
                        ui.indent();
                        ui.text(format!(
                            "  BoardID: 0x{:04X}  Uptime: {}s  EngineType: 0x{:04X}",
                            b.board_id, b.seconds_since_reset, b.engine_type
                        ));
                        ui.unindent();
                    }

                    if let Some(ref b) = self.button_counters {
                        ui.text("ButtonCounters (0x770004):");
                        ui.indent();
                        ui.text(format!(
                            "  Brake: {}  Clutch: {}  AC: {}",
                            b.brake_pedal, b.clutch_up, b.ac_button
                        ));
                        ui.unindent();
                    }

                    if let Some(ref m) = self.io_meta {
                        ui.text("IoMetaInfo (0x770005):");
                        ui.indent();
                        ui.text(format!(
                            "  Outputs: {}  LowSide: {}  DC: {}",
                            m.outputs_count, m.low_side_count, m.dc_outputs_count
                        ));
                        ui.unindent();
                    }

                    if let Some(ref a) = self.raw_analog2 {
                        ui.text("RawAnalog2 (0x770006) [V]:");
                        ui.indent();
                        ui.text(format!(
                            "  TPS2: {:.2}/{:.2}  AuxLin: {:.2}/{:.2}",
                            a.tps2_primary, a.tps2_secondary, a.aux_linear1, a.aux_linear2
                        ));
                        ui.text(format!(
                            "  Oil: {:.2}  Fuel: {:.2}/{:.2}  AuxTemp: {:.2}",
                            a.oil_pressure, a.fuel_pressure_low, a.fuel_pressure_high, a.aux_temp1
                        ));
                        ui.unindent();
                    }

                    if let Some(ref p) = self.pin_state {
                        ui.text("PinState (0x770007):");
                        ui.indent();
                        ui.text(format!(
                            "  Toggles: {}  State0: {}ms  State1: {}ms",
                            p.pin_toggle_counter, p.duration_state0_ms, p.duration_state1_ms
                        ));
                        ui.unindent();
                    }

                    if let Some(ref a) = self.aux_digital {
                        ui.text("AuxDigitalCounters (0x770008):");
                        ui.indent();
                        ui.text(format!("  Lua 0-7: {:?}", a.lua_digital));
                        ui.unindent();
                    }

                    if let Some(ref a) = self.raw_lua_analog1 {
                        ui.text("RawLuaAnalog1 (0x770013) [V]:");
                        ui.indent();
                        ui.text(format!(
                            "  Aux 1-8: {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} {:.2}",
                            a.aux_analog[0],
                            a.aux_analog[1],
                            a.aux_analog[2],
                            a.aux_analog[3],
                            a.aux_analog[4],
                            a.aux_analog[5],
                            a.aux_analog[6],
                            a.aux_analog[7]
                        ));
                        ui.unindent();
                    }

                    if let Some(ref c) = self.ecu_calibration {
                        ui.text("EcuGetCalibration (0x770010):");
                        ui.indent();
                        ui.text(format!("  Hash: 0x{:08X}  Value: {}", c.hash, c.value));
                        ui.unindent();
                    }
                }

                ui.separator();
                ui.text_disabled("Protocol: rusEFI Bench Test, base 0x770000");
            });
    }
}
