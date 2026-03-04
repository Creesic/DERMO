//! rusEFI GDI-4ch CAN Plugin
//!
//! Implements the rusEFI GDI-4ch protocol from rusefi-hardware/GDI-4ch/firmware.
//! See can.cpp, persistence.h, and GDI-STM gdi_can_protocol.cpp.
//!
//! Protocol summary:
//! - GDI TX (we receive): Extended IDs 0xBB20 + offset. Status, config 1-4, version, SENT.
//! - ECU TX (we send): Extended IDs 0xBB30 + offset. Config packets with tag 0x78, DLC=8.
//! - 500 kbps, little-endian, float values use fixed-point ×128.

use crate::core::CanMessage;
use crate::plugins::Plugin;
use imgui::{Condition, Ui};
use std::collections::HashMap;
use std::time::Instant;
use tracing::info;

// Protocol constants (from GDI-4ch can.cpp, GDI-STM gdi_can_protocol.cpp)
const GDI4_BASE_ADDRESS: u32 = 0xBB20;
const GDI4_CHANGE_ADDRESS: u32 = 0xBB30;
const GDI4_MAGIC: u8 = 0x67;
const GDI4_CAN_SET_TAG: u8 = 0x78;
const GDI4_CAN_SET_DLC: usize = 8;
const GDI4_FIXED_POINT: f32 = 128.0;

// SENT scaling (from can.cpp comments: 0.1 bar, 0.01 deg C)
const GM_SENT_SCALE_PRESSURE: f32 = 10.0;   // raw / 10 = bar
const GM_SENT_SCALE_TEMPERATURE: f32 = 100.0; // raw / 100 = deg C

fn float_to_short128(v: f32) -> u16 {
    (v * GDI4_FIXED_POINT).round() as u16
}

fn short_to_float128(v: u16) -> f32 {
    v as f32 / GDI4_FIXED_POINT
}

fn get_u16_le(data: &[u8], offset: usize) -> u16 {
    if offset + 2 > data.len() {
        return 0;
    }
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn get_i16_le(data: &[u8], offset: usize) -> i16 {
    if offset + 2 > data.len() {
        return 0;
    }
    i16::from_le_bytes([data[offset], data[offset + 1]])
}

fn set_u16_le(data: &mut [u8], offset: usize, v: u16) {
    if offset + 2 <= data.len() {
        data[offset..offset + 2].copy_from_slice(&v.to_le_bytes());
    }
}

/// Status from GDI (ID+0)
#[derive(Clone, Debug, Default)]
struct GdiStatus {
    input_can_id: u8,
    update_counter: u8,
    is_happy: bool,
    fault: u8,
    magic: u8,
    last_seen: Option<Instant>,
}

/// Config packet 1 (ID+1): BoostVoltage, BoostCurrent, TBoostMin, TBoostMax
#[derive(Clone, Debug, Default)]
struct GdiConfig1 {
    boost_voltage: u16,
    boost_current: f32,
    t_boost_min: u16,
    t_boost_max: u16,
}

/// Config packet 2 (ID+2): PeakCurrent, TpeakDuration, TpeakOff, Tbypass
#[derive(Clone, Debug, Default)]
struct GdiConfig2 {
    peak_current: f32,
    t_peak_duration: u16,
    t_peak_off: u16,
    t_bypass: u16,
}

/// Config packet 3 (ID+3): HoldCurrent, TholdOff, THoldDuration, PumpPeakCurrent
#[derive(Clone, Debug, Default)]
struct GdiConfig3 {
    hold_current: f32,
    t_hold_off: u16,
    t_hold_duration: u16,
    pump_peak_current: f32,
}

/// Config packet 4 (ID+4): PumpHoldCurrent, outputCanID
#[derive(Clone, Debug, Default)]
struct GdiConfig4 {
    pump_hold_current: f32,
    output_can_id: u16,
}

/// Version (ID+5) - VERSION = {year/100, year%100, month, day}
#[derive(Clone, Debug, Default)]
struct GdiVersion {
    year: u16,  // full year e.g. 2025
    month: u8,
    day: u8,
}

/// SENT (ID+6): pressure, temperature
#[derive(Clone, Debug, Default)]
struct GdiSent {
    pressure_bar: f32,
    temp_c: f32,
}

/// Per-chip data (GDI-4ch can have multiple chips with offset IDs)
#[derive(Clone, Debug, Default)]
struct GdiChipData {
    status: Option<GdiStatus>,
    config1: Option<GdiConfig1>,
    config2: Option<GdiConfig2>,
    config3: Option<GdiConfig3>,
    config4: Option<GdiConfig4>,
    version: Option<GdiVersion>,
    sent: Option<GdiSent>,
}

pub struct RusefiGdiPlugin {
    /// Chip index -> data (chip 0 = base ID, chip 1 = base+0x10, etc.)
    chip_data: HashMap<u8, GdiChipData>,
    /// Base output ID (GDI TX base, e.g. 0xBB20)
    output_base: u32,
    /// Base input ID (ECU TX base, e.g. 0xBB30)
    input_base: u32,
    /// TX bus
    tx_bus: u8,
    /// Editable config (for sending)
    edit_config1: GdiConfig1,
    edit_config2: GdiConfig2,
    edit_config3: GdiConfig3,
    edit_config4: GdiConfig4,
    edit_output_can_id: u16,
    /// Last apply time
    last_apply: Option<Instant>,
    /// Chip index to target when sending (0 = first)
    target_chip: u8,
}

impl RusefiGdiPlugin {
    pub fn new() -> Self {
        let mut edit_config1 = GdiConfig1::default();
        edit_config1.boost_voltage = 65;
        edit_config1.boost_current = 13.0;
        edit_config1.t_boost_min = 100;
        edit_config1.t_boost_max = 400;

        let mut edit_config2 = GdiConfig2::default();
        edit_config2.peak_current = 9.4;
        edit_config2.t_peak_duration = 700;
        edit_config2.t_peak_off = 10;
        edit_config2.t_bypass = 10;

        let mut edit_config3 = GdiConfig3::default();
        edit_config3.hold_current = 3.7;
        edit_config3.t_hold_off = 60;
        edit_config3.t_hold_duration = 10000;
        edit_config3.pump_peak_current = 5.0;

        let mut edit_config4 = GdiConfig4::default();
        edit_config4.pump_hold_current = 3.0;
        edit_config4.output_can_id = GDI4_BASE_ADDRESS as u16;

        Self {
            chip_data: HashMap::new(),
            output_base: GDI4_BASE_ADDRESS,
            input_base: GDI4_CHANGE_ADDRESS,
            tx_bus: 0,
            edit_config1,
            edit_config2,
            edit_config3,
            edit_config4,
            edit_output_can_id: GDI4_BASE_ADDRESS as u16,
            last_apply: None,
            target_chip: 0,
        }
    }

    fn chip_index_from_id(&self, id: u32) -> Option<u8> {
        if id < self.output_base {
            return None;
        }
        let offset = id - self.output_base;
        if offset > 0x7F {
            return None;
        }
        Some((offset / 0x10) as u8)
    }

    fn base_for_chip(&self, chip: u8) -> u32 {
        self.output_base + (chip as u32) * 0x10
    }

    fn process_message(&mut self, msg: &crate::hardware::can_manager::ManagerMessage) {
        let id = msg.message.id;
        let data = &msg.message.data;

        // Must be extended ID in our range (output_base to output_base + 0x50 for multiple chips)
        if id < self.output_base || id > self.output_base + 0x50 {
            return;
        }

        let chip = self.chip_index_from_id(id).unwrap_or(0);
        let base = self.base_for_chip(chip);
        let sub_id = id - base;
        let entry = self.chip_data.entry(chip).or_default();

        let now = Instant::now();

        match sub_id {
            0 => {
                // Status
                if data.len() >= 8 {
                    entry.status = Some(GdiStatus {
                        input_can_id: data[0],
                        update_counter: data[1],
                        is_happy: data[2] != 0,
                        fault: data[6],
                        magic: data[7],
                        last_seen: Some(now),
                    });
                }
            }
            1 => {
                // Config1
                if data.len() >= 8 {
                    entry.config1 = Some(GdiConfig1 {
                        boost_voltage: get_u16_le(data, 0),
                        boost_current: short_to_float128(get_u16_le(data, 2)),
                        t_boost_min: get_u16_le(data, 4),
                        t_boost_max: get_u16_le(data, 6),
                    });
                }
            }
            2 => {
                if data.len() >= 8 {
                    entry.config2 = Some(GdiConfig2 {
                        peak_current: short_to_float128(get_u16_le(data, 0)),
                        t_peak_duration: get_u16_le(data, 2),
                        t_peak_off: get_u16_le(data, 4),
                        t_bypass: get_u16_le(data, 6),
                    });
                }
            }
            3 => {
                if data.len() >= 8 {
                    entry.config3 = Some(GdiConfig3 {
                        hold_current: short_to_float128(get_u16_le(data, 0)),
                        t_hold_off: get_u16_le(data, 2),
                        t_hold_duration: get_u16_le(data, 4),
                        pump_peak_current: short_to_float128(get_u16_le(data, 6)),
                    });
                }
            }
            4 => {
                // GDI-4ch sends config4 with DLC=2 (only pump_hold_current)
                if data.len() >= 2 {
                    entry.config4 = Some(GdiConfig4 {
                        pump_hold_current: short_to_float128(get_u16_le(data, 0)),
                        output_can_id: if data.len() >= 4 { get_u16_le(data, 2) } else { 0 },
                    });
                }
            }
            5 => {
                if data.len() >= 4 {
                    let year_hi = data[0] as u16;
                    let year_lo = data[1] as u16;
                    entry.version = Some(GdiVersion {
                        year: year_hi * 100 + year_lo,
                        month: data[2],
                        day: data[3],
                    });
                }
            }
            6 => {
                if data.len() >= 8 {
                    let press_raw = get_u16_le(data, 0);
                    let temp_raw = get_i16_le(data, 2);
                    entry.sent = Some(GdiSent {
                        pressure_bar: press_raw as f32 / GM_SENT_SCALE_PRESSURE,
                        temp_c: temp_raw as f32 / GM_SENT_SCALE_TEMPERATURE,
                    });
                }
            }
            _ => {}
        }
    }

    fn sync_edit_from_received(&mut self, chip: u8) {
        if let Some(d) = self.chip_data.get(&chip) {
            if let Some(c) = &d.config1 {
                self.edit_config1 = c.clone();
            }
            if let Some(c) = &d.config2 {
                self.edit_config2 = c.clone();
            }
            if let Some(c) = &d.config3 {
                self.edit_config3 = c.clone();
            }
            if let Some(c) = &d.config4 {
                self.edit_config4 = c.clone();
                self.edit_output_can_id = c.output_can_id;
            }
        }
    }

    fn queue_config_packets(&self, ctx: &mut crate::plugins::PluginContext) {
        let input_id = self.input_base + (self.target_chip as u32) * 0x10;

        let mut p0 = vec![GDI4_CAN_SET_TAG; GDI4_CAN_SET_DLC];
        set_u16_le(&mut p0, 1, self.edit_config1.boost_voltage);
        set_u16_le(&mut p0, 3, float_to_short128(self.edit_config1.boost_current));
        set_u16_le(&mut p0, 5, self.edit_config1.t_boost_min);
        ctx.queue_send.push((self.tx_bus, CanMessage::new(self.tx_bus, input_id, p0.into())));

        let mut p1 = vec![GDI4_CAN_SET_TAG; GDI4_CAN_SET_DLC];
        set_u16_le(&mut p1, 1, self.edit_config1.t_boost_max);
        set_u16_le(&mut p1, 3, float_to_short128(self.edit_config2.peak_current));
        set_u16_le(&mut p1, 5, self.edit_config2.t_peak_duration);
        ctx.queue_send.push((self.tx_bus, CanMessage::new(self.tx_bus, input_id + 1, p1.into())));

        let mut p2 = vec![GDI4_CAN_SET_TAG; GDI4_CAN_SET_DLC];
        set_u16_le(&mut p2, 1, self.edit_config2.t_peak_off);
        set_u16_le(&mut p2, 3, self.edit_config2.t_bypass);
        set_u16_le(&mut p2, 5, float_to_short128(self.edit_config3.hold_current));
        ctx.queue_send.push((self.tx_bus, CanMessage::new(self.tx_bus, input_id + 2, p2.into())));

        let mut p3 = vec![GDI4_CAN_SET_TAG; GDI4_CAN_SET_DLC];
        set_u16_le(&mut p3, 1, self.edit_config3.t_hold_off);
        set_u16_le(&mut p3, 3, self.edit_config3.t_hold_duration);
        set_u16_le(&mut p3, 5, float_to_short128(self.edit_config3.pump_peak_current));
        ctx.queue_send.push((self.tx_bus, CanMessage::new(self.tx_bus, input_id + 3, p3.into())));

        let mut p4 = vec![GDI4_CAN_SET_TAG; GDI4_CAN_SET_DLC];
        set_u16_le(&mut p4, 1, float_to_short128(self.edit_config4.pump_hold_current));
        set_u16_le(&mut p4, 3, self.edit_output_can_id);
        ctx.queue_send.push((self.tx_bus, CanMessage::new(self.tx_bus, input_id + 4, p4.into())));

        info!("GDI: Sent config to 0x{:03X}", input_id);
    }
}

impl Plugin for RusefiGdiPlugin {
    fn id(&self) -> &str {
        "rusefi_gdi"
    }

    fn name(&self) -> &str {
        "rusEFI GDI"
    }

    fn description(&self) -> &str {
        "rusEFI GDI-4ch - PT2001 injector driver config and status"
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

        ui.window("rusEFI GDI-4ch")
            .size([480.0, 560.0], Condition::FirstUseEver)
            .position([100.0, 120.0], Condition::FirstUseEver)
            .opened(is_open)
            .build(|| {
                if !ctx.is_connected && !ctx.has_playback {
                    ui.text_colored([1.0, 0.5, 0.3, 1.0], "No CAN interface connected");
                    ui.text_wrapped("Connect to a CAN interface in Hardware Manager first, or open a CAN log for playback. Sending config requires a live connection.");
                    return;
                }
                if ctx.has_playback && !ctx.is_connected {
                    ui.text_colored([0.5, 0.8, 0.5, 1.0], "Playback mode");
                    ui.text_wrapped("Showing GDI data from loaded log. Config send requires a live connection.");
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

                    ui.text("Output base ID (GDI TX):");
                    ui.same_line();
                    let mut out_hex = format!("0x{:04X}", self.output_base);
                    if ui.input_text("##out_base", &mut out_hex).build() {
                        if let Ok(n) = u32::from_str_radix(out_hex.trim().trim_start_matches("0x"), 16) {
                            self.output_base = n;
                        }
                    }

                    ui.text("Input base ID (ECU TX):");
                    ui.same_line();
                    let mut in_hex = format!("0x{:04X}", self.input_base);
                    if ui.input_text("##in_base", &mut in_hex).build() {
                        if let Ok(n) = u32::from_str_radix(in_hex.trim().trim_start_matches("0x"), 16) {
                            self.input_base = n;
                        }
                    }

                    ui.text("Target chip:");
                    ui.same_line();
                    let mut tc = self.target_chip as i32;
                    if ui.input_int("##target_chip", &mut tc).build() {
                        self.target_chip = tc.clamp(0, 15) as u8;
                    }

                    ui.separator();
                    ui.text("Send config to GDI");
                    ui.separator();

                    if ui.button("Load from received") {
                        self.sync_edit_from_received(self.target_chip);
                    }
                    ui.same_line();
                    if ui.button("Apply config") {
                        self.queue_config_packets(ctx);
                        self.last_apply = Some(Instant::now());
                    }
                    if let Some(t) = self.last_apply {
                        if t.elapsed().as_secs() < 2 {
                            ui.same_line();
                            ui.text_colored([0.3, 0.8, 0.3, 1.0], "Sent");
                        }
                    }

                    ui.separator();
                    ui.text("Edit config (values sent on Apply)");
                    ui.separator();

                    ui.text("Boost:");
                    ui.indent();
                    let mut bv = self.edit_config1.boost_voltage as i32;
                    if ui.input_int("Voltage (0-100)", &mut bv).build() {
                        self.edit_config1.boost_voltage = bv.clamp(0, 100) as u16;
                    }
                    let mut bc = self.edit_config1.boost_current;
                    if ui.input_float("Current (A)", &mut bc).build() {
                        self.edit_config1.boost_current = bc.max(0.0);
                    }
                    let mut tbm = self.edit_config1.t_boost_min as i32;
                    if ui.input_int("TBoostMin (us)", &mut tbm).build() {
                        self.edit_config1.t_boost_min = tbm.max(0) as u16;
                    }
                    let mut tb_max = self.edit_config1.t_boost_max as i32;
                    if ui.input_int("TBoostMax (us)", &mut tb_max).build() {
                        self.edit_config1.t_boost_max = tb_max.max(0) as u16;
                    }
                    ui.unindent();

                    ui.text("Peak:");
                    ui.indent();
                    let mut pc = self.edit_config2.peak_current;
                    if ui.input_float("PeakCurrent (A)", &mut pc).build() {
                        self.edit_config2.peak_current = pc.max(0.0);
                    }
                    let mut tpd = self.edit_config2.t_peak_duration as i32;
                    if ui.input_int("TpeakDuration (us)", &mut tpd).build() {
                        self.edit_config2.t_peak_duration = tpd.max(0) as u16;
                    }
                    let mut tpo = self.edit_config2.t_peak_off as i32;
                    if ui.input_int("TpeakOff (us)", &mut tpo).build() {
                        self.edit_config2.t_peak_off = tpo.max(0) as u16;
                    }
                    let mut tb = self.edit_config2.t_bypass as i32;
                    if ui.input_int("Tbypass (us)", &mut tb).build() {
                        self.edit_config2.t_bypass = tb.max(0) as u16;
                    }
                    ui.unindent();

                    ui.text("Hold:");
                    ui.indent();
                    let mut hc = self.edit_config3.hold_current;
                    if ui.input_float("HoldCurrent (A)", &mut hc).build() {
                        self.edit_config3.hold_current = hc.max(0.0);
                    }
                    let mut tho = self.edit_config3.t_hold_off as i32;
                    if ui.input_int("TholdOff (us)", &mut tho).build() {
                        self.edit_config3.t_hold_off = tho.max(0) as u16;
                    }
                    let mut thd = self.edit_config3.t_hold_duration as i32;
                    if ui.input_int("THoldDuration (us)", &mut thd).build() {
                        self.edit_config3.t_hold_duration = thd.max(0) as u16;
                    }
                    let mut ppc = self.edit_config3.pump_peak_current;
                    if ui.input_float("PumpPeakCurrent (A)", &mut ppc).build() {
                        self.edit_config3.pump_peak_current = ppc.max(0.0);
                    }
                    ui.unindent();

                    ui.text("Pump:");
                    ui.indent();
                    let mut phc = self.edit_config4.pump_hold_current;
                    if ui.input_float("PumpHoldCurrent (A)", &mut phc).build() {
                        self.edit_config4.pump_hold_current = phc.max(0.0);
                    }
                    let mut oid = self.edit_output_can_id as i32;
                    if ui.input_int("OutputCanID", &mut oid).build() {
                        self.edit_output_can_id = oid.clamp(0, 0xFFFF) as u16;
                    }
                    ui.unindent();

                    ui.separator();
                }

                ui.text("Received data");
                ui.separator();

                let mut chips: Vec<u8> = self.chip_data.keys().copied().collect();
                chips.sort();

                if chips.is_empty() {
                    ui.text_colored([0.6, 0.6, 0.6, 1.0], "No GDI data received yet");
                    ui.text_wrapped("GDI TX uses extended IDs 0xBB20 + offset. Ensure 500kbps, extended frames.");
                } else {
                    for chip in chips {
                        let d = self.chip_data.get(&chip).unwrap();
                        ui.text_colored([0.5, 0.8, 0.5, 1.0], format!("Chip {}", chip));

                        if let Some(s) = &d.status {
                            let happy_color = if s.is_happy {
                                [0.3, 0.8, 0.3, 1.0]
                            } else {
                                [1.0, 0.3, 0.3, 1.0]
                            };
                            ui.text_colored(
                                happy_color,
                                format!("  Status: {}  Fault: {}  Magic: 0x{:02X}", s.is_happy, s.fault, s.magic),
                            );
                        }
                        if let Some(c) = &d.config1 {
                            ui.text(format!("  Boost: {}V {}A  TBoost: {}-{} us", c.boost_voltage, c.boost_current, c.t_boost_min, c.t_boost_max));
                        }
                        if let Some(c) = &d.config2 {
                            ui.text(format!("  Peak: {}A  Tpeak: {} us  Tbypass: {} us", c.peak_current, c.t_peak_duration, c.t_bypass));
                        }
                        if let Some(c) = &d.config3 {
                            ui.text(format!("  Hold: {}A  Thold: {} us  PumpPeak: {}A", c.hold_current, c.t_hold_duration, c.pump_peak_current));
                        }
                        if let Some(c) = &d.config4 {
                            ui.text(format!("  PumpHold: {}A  OutputID: 0x{:04X}", c.pump_hold_current, c.output_can_id));
                        }
                        if let Some(v) = &d.version {
                            ui.text(format!("  Version: {}/{:02}/{:02}", v.year, v.month, v.day));
                        }
                        if let Some(s) = &d.sent {
                            ui.text(format!("  SENT: {:.2} bar  {:.1} °C", s.pressure_bar, s.temp_c));
                        }
                    }
                }

                ui.separator();
                ui.text_disabled("Protocol: rusEFI GDI-4ch, 500kbps, ext IDs 0xBB20+");
            });
    }
}
