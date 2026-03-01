use imgui::{Condition, StyleColor, Ui};
use crate::core::dbc::{DbcFile, DbcMessage, DbcSignal, ByteOrder, ValueType};
use crate::decode::decoder::extract_bits;
use std::cell::RefCell;

/// Signal color palette for visualizing different signals (more vibrant)
const SIGNAL_COLORS: [[f32; 4]; 10] = [
    [0.3, 0.5, 0.9, 0.7],  // Blue
    [0.3, 0.7, 0.4, 0.7],  // Green
    [0.9, 0.6, 0.2, 0.7],  // Orange
    [0.7, 0.4, 0.8, 0.7],  // Purple
    [0.8, 0.3, 0.4, 0.7],  // Red
    [0.3, 0.8, 0.8, 0.7],  // Cyan
    [0.8, 0.8, 0.3, 0.7],  // Yellow
    [0.6, 0.4, 0.3, 0.7],  // Brown
    [0.5, 0.5, 0.7, 0.7],  // Slate
    [0.7, 0.5, 0.7, 0.7],  // Mauve
];

/// Callback type for when a signal is created
pub type SignalCreatedCallback = Box<dyn FnMut(u32, DbcSignal)>;

/// Callback type for toggling a signal on the chart
pub type ToggleChartCallback = Box<dyn FnMut(&str)>;

/// State for a single quadrant in the 4-panel bit visualizer
#[derive(Clone)]
struct QuadrantState {
    selected_message_id: Option<u32>,
    selected_bus: Option<u8>,
    current_data: [u8; 8],
    bit_flip_counts: [u32; 64],
    last_data: [u8; 8],
    max_flip_count: u32,
    selection_start: Option<usize>,
    selection_end: Option<usize>,
    is_dragging: bool,
}

impl QuadrantState {
    fn new() -> Self {
        Self {
            selected_message_id: None,
            selected_bus: None,
            current_data: [0; 8],
            bit_flip_counts: [0; 64],
            last_data: [0; 8],
            max_flip_count: 0,
            selection_start: None,
            selection_end: None,
            is_dragging: false,
        }
    }

    fn update_message(&mut self, id: u32, bus: u8, data: &[u8]) {
        let is_different = match (self.selected_message_id, self.selected_bus) {
            (Some(current_id), Some(current_bus)) => id != current_id || bus != current_bus,
            _ => true,
        };
        if is_different {
            self.selected_message_id = Some(id);
            self.selected_bus = Some(bus);
            let old_data = self.last_data;
            let mut padded_new: [u8; 8] = [0; 8];
            for (i, &byte) in data.iter().enumerate() {
                if i < 8 {
                    padded_new[i] = byte;
                }
            }
            self.update_activity(&old_data, &padded_new);
        }
        self.last_data = self.current_data;
        self.current_data = [0; 8];
        for (i, &byte) in data.iter().enumerate() {
            if i < 8 {
                self.current_data[i] = byte;
            }
        }
    }

    fn update_activity(&mut self, old_data: &[u8; 8], new_data: &[u8; 8]) {
        for byte_idx in 0..8 {
            let changed = old_data[byte_idx] ^ new_data[byte_idx];
            for bit_idx in 0..8 {
                if (changed >> bit_idx) & 1 == 1 {
                    let abs_bit = byte_idx * 8 + (7 - bit_idx);
                    self.bit_flip_counts[abs_bit] += 1;
                    self.max_flip_count = self.max_flip_count.max(self.bit_flip_counts[abs_bit]);
                }
            }
        }
    }

    fn reset_activity(&mut self) {
        self.bit_flip_counts = [0; 64];
        self.max_flip_count = 0;
    }

    fn clear(&mut self) {
        self.selected_message_id = None;
        self.selected_bus = None;
        self.current_data = [0; 8];
        self.selection_start = None;
        self.selection_end = None;
        self.is_dragging = false;
    }

    /// Set selection for savestate restore (data will be populated when messages arrive)
    fn set_selection(&mut self, id: u32, bus: u8) {
        self.selected_message_id = Some(id);
        self.selected_bus = Some(bus);
    }
}

/// Window for visualizing CAN message bytes and bits in a grid format (4 quadrants)
pub struct BitVisualizerWindow {
    quadrants: [QuadrantState; 4],
    /// Which quadrant receives the next message selection from the list (0-3)
    focused_quadrant: usize,
    /// Show signal overlays
    show_signals: bool,

    // Signal creation dialog
    show_create_dialog: bool,
    create_quadrant: Option<usize>,
    new_signal_name: String,
    new_signal_is_signed: bool,
    new_signal_is_little_endian: bool,
    new_signal_factor: String,
    new_signal_offset: String,
    new_signal_unit: String,
    signal_counter: u32,

    // Signal editing
    show_edit_dialog: bool,
    edit_quadrant: Option<usize>,
    editing_signal_name: String,
    editing_signal_idx: Option<usize>,
    edit_start_bit: u8,
    edit_bit_length: u8,
    edit_is_signed: bool,
    edit_is_little_endian: bool,
    edit_factor: String,
    edit_offset: String,
    edit_unit: String,

    // Callbacks
    on_signal_created: RefCell<Option<SignalCreatedCallback>>,
    on_toggle_chart: RefCell<Option<ToggleChartCallback>>,
    charted_signals: RefCell<Vec<String>>,
    chart_toggle_request: RefCell<Option<String>>,
}

impl BitVisualizerWindow {
    pub fn new() -> Self {
        Self {
            quadrants: [
                QuadrantState::new(),
                QuadrantState::new(),
                QuadrantState::new(),
                QuadrantState::new(),
            ],
            focused_quadrant: 0,
            show_signals: true,
            show_create_dialog: false,
            create_quadrant: None,
            new_signal_name: String::new(),
            new_signal_is_signed: false,
            new_signal_is_little_endian: true,
            new_signal_factor: String::from("1"),
            new_signal_offset: String::from("0"),
            new_signal_unit: String::new(),
            signal_counter: 0,
            show_edit_dialog: false,
            edit_quadrant: None,
            editing_signal_name: String::new(),
            editing_signal_idx: None,
            edit_start_bit: 0,
            edit_bit_length: 1,
            edit_is_signed: false,
            edit_is_little_endian: true,
            edit_factor: String::from("1"),
            edit_offset: String::from("0"),
            edit_unit: String::new(),
            on_signal_created: RefCell::new(None),
            on_toggle_chart: RefCell::new(None),
            charted_signals: RefCell::new(Vec::new()),
            chart_toggle_request: RefCell::new(None),
        }
    }

    pub fn set_on_signal_created<F>(&self, callback: F)
    where
        F: FnMut(u32, DbcSignal) + 'static,
    {
        *self.on_signal_created.borrow_mut() = Some(Box::new(callback));
    }

    /// Set callback for toggling signals on charts
    pub fn set_on_toggle_chart<F>(&self, callback: F)
    where
        F: FnMut(&str) + 'static,
    {
        *self.on_toggle_chart.borrow_mut() = Some(Box::new(callback));
    }

    /// Update the list of charted signals
    pub fn set_charted_signals(&self, signals: Vec<String>) {
        *self.charted_signals.borrow_mut() = signals;
    }

    /// Check if a signal on the given bus is charted
    fn is_signal_charted(&self, signal_name: &str, bus: u8) -> bool {
        let key = format!("{}@bus{}", signal_name, bus);
        self.charted_signals.borrow().contains(&key)
    }

    /// Check if there's a pending chart toggle request and return the signal name
    pub fn take_chart_toggle_request(&self) -> Option<String> {
        self.chart_toggle_request.borrow_mut().take()
    }

    /// Request to toggle a signal on the chart
    fn request_chart_toggle(&self, signal_name: String, bus: u8) {
        let key = format!("{}@bus{}", signal_name, bus);
        *self.chart_toggle_request.borrow_mut() = Some(key);
    }

    /// Get the currently selected (message_id, bus) from the focused quadrant
    pub fn get_selected(&self) -> Option<(u32, u8)> {
        let q = &self.quadrants[self.focused_quadrant];
        match (q.selected_message_id, q.selected_bus) {
            (Some(id), Some(bus)) => Some((id, bus)),
            _ => None,
        }
    }

    /// Set focused quadrant's message (called when user selects from message list)
    pub fn set_message(&mut self, id: u32, bus: u8, data: &[u8]) {
        self.quadrants[self.focused_quadrant].update_message(id, bus, data);
    }

    /// Update data for any quadrant displaying this (id, bus) - for playback of all quadrants
    pub fn update_message_data(&mut self, id: u32, bus: u8, data: &[u8]) {
        for q in &mut self.quadrants {
            if q.selected_message_id == Some(id) && q.selected_bus == Some(bus) {
                q.update_message(id, bus, data);
            }
        }
    }

    /// Get all (id, bus) pairs that quadrants are displaying
    pub fn quadrant_messages(&self) -> Vec<(u32, u8)> {
        self.quadrants
            .iter()
            .filter_map(|q| {
                match (q.selected_message_id, q.selected_bus) {
                    (Some(id), Some(bus)) => Some((id, bus)),
                    _ => None,
                }
            })
            .collect()
    }

    /// Get quadrant selections for savestate (up to 4 entries, empty quadrants omitted)
    pub fn get_quadrant_selections(&self) -> Vec<(u32, u8)> {
        self.quadrants
            .iter()
            .filter_map(|q| match (q.selected_message_id, q.selected_bus) {
                (Some(id), Some(bus)) => Some((id, bus)),
                _ => None,
            })
            .collect()
    }

    /// Restore quadrant selections from savestate (data populated when messages arrive)
    pub fn set_quadrant_selections(&mut self, selections: &[(u32, u8)]) {
        for (q, sel) in self.quadrants.iter_mut().zip(selections.iter().take(4)) {
            q.set_selection(sel.0, sel.1);
        }
    }

    pub fn render(&mut self, ui: &Ui, dbc: &mut DbcFile, is_open: &mut bool) {
        ui.window("Bit Visualizer")
            .size([900.0, 700.0], Condition::FirstUseEver)
            .position([100.0, 100.0], Condition::FirstUseEver)
            .opened(is_open)
            .build(|| {
                self.render_content(ui, dbc);
            });

        if self.show_create_dialog {
            self.render_create_dialog(ui, dbc);
        }

        if self.show_edit_dialog {
            self.render_edit_dialog(ui, dbc);
        }
    }

    fn render_content(&mut self, ui: &Ui, dbc: &mut DbcFile) {
        ui.checkbox("Show Signal Colors", &mut self.show_signals);
        ui.same_line();
        ui.text_colored([0.6, 0.6, 0.6, 1.0], "Click a quadrant to focus it, then select a message from the list");
        ui.separator();

        // 2x2 layout: each quadrant gets ~half width and half height
        let avail = ui.content_region_avail();
        let quad_w = (avail[0] - 8.0) / 2.0;  // 8px gap between columns
        let quad_h = (avail[1] - 8.0) / 2.0;  // 8px gap between rows

        for row in 0..2 {
            ui.columns(2, "quad_cols", false);
            ui.set_column_width(0, quad_w);
            ui.set_column_width(1, quad_w);
            for col in 0..2 {
                let idx = row * 2 + col;
                ui.child_window(format!("quad_{}", idx))
                    .size([quad_w, quad_h])
                    .border(true)
                    .build(|| {
                        self.render_quadrant(ui, dbc, idx);
                    });
                ui.next_column();
            }
            ui.columns(1, "", false);
        }
    }

    fn render_quadrant(&mut self, ui: &Ui, dbc: &mut DbcFile, idx: usize) {
        let q = &mut self.quadrants[idx];
        let is_focused = self.focused_quadrant == idx;

        // Header: click to focus, message info, clear/reset
        if let Some(id) = q.selected_message_id {
            let bus = q.selected_bus.unwrap_or(0);
            let header = format!("{}. 0x{:03X} [Bus {}]", idx + 1, id, bus);
            let header_color = if is_focused { [0.3, 0.6, 0.9, 1.0] } else { [0.6, 0.6, 0.6, 1.0] };
            let _tok = ui.push_style_color(StyleColor::Text, header_color);
            if ui.selectable(&format!("{}##qh{}", header, idx)) {
                self.focused_quadrant = idx;
            }
            drop(_tok);
            if ui.is_item_hovered() {
                ui.tooltip(|| {
                    ui.text(if is_focused { "Focused (receives new selections)" } else { "Click to focus" });
                });
            }
            ui.same_line();
            if ui.small_button(&format!("Clear##q{}", idx)) {
                q.clear();
            }
            ui.same_line();
            if ui.small_button(&format!("Reset##q{}", idx)) {
                q.reset_activity();
            }
            if let Some(msg_def) = dbc.get_message(id) {
                ui.same_line();
                ui.text_colored([0.5, 0.8, 0.5, 1.0], &format!("({})", msg_def.name));
            }
        } else {
            let label = format!("{}. 0x--- [--]  (click to focus, select message)", idx + 1);
            let _tok = ui.push_style_color(StyleColor::Text, [0.5, 0.5, 0.5, 1.0]);
            if ui.selectable(&label) {
                self.focused_quadrant = idx;
            }
            drop(_tok);
        }

        ui.separator();

        if q.selected_message_id.is_none() {
            ui.text_colored([0.5, 0.5, 0.5, 1.0], "Select a message from the Messages list");
            return;
        }

        self.render_bit_grid_quadrant(ui, dbc, idx);
        ui.separator();
        self.render_decoded_signals_quadrant(ui, dbc, idx);
    }

    fn render_bit_grid_quadrant(&mut self, ui: &Ui, dbc: &DbcFile, idx: usize) {
        let signals = self.get_signal_info_quadrant(dbc, idx);
        let selection_bits = self.get_selection_bits_quadrant(idx);
        let mut bit_rects: Vec<(usize, [f32; 2], [f32; 2])> = Vec::new();
        let mut header_positions: Vec<[f32; 2]> = Vec::new();

        for byte_idx in 0..8 {
            let byte_val = self.quadrants[idx].current_data[byte_idx];

            ui.text(format!("B{}:", byte_idx));
            ui.same_line();

            for bit_idx in (0..8).rev() {
                let bit_val = (byte_val >> bit_idx) & 1;
                let abs_bit_pos = byte_idx * 8 + (7 - bit_idx);

                let (mut bg_color, signal_name, is_msb, is_lsb) = if self.show_signals {
                    self.get_bit_signal_info(abs_bit_pos, &signals)
                } else {
                    ([0.3, 0.3, 0.3, 1.0], None, false, false)
                };

                if !self.show_signals {
                    let activity = self.get_bit_activity_quadrant(idx, abs_bit_pos);
                    if activity > 0.0 {
                        bg_color[0] = (bg_color[0] + activity * 0.4).min(1.0);
                        bg_color[1] = (bg_color[1] + activity * 0.2).min(1.0);
                    }
                }

                let is_selected = selection_bits.contains(&abs_bit_pos);
                let indicator = if is_msb { "M" } else if is_lsb { "L" } else { " " };
                let button_label = format!("{}{}##q{}b{}", bit_val, indicator, idx, abs_bit_pos);

                let _color_token = ui.push_style_color(StyleColor::Button, bg_color);
                let _hover_token = ui.push_style_color(StyleColor::ButtonHovered, [
                    (bg_color[0] + 0.2).min(1.0), (bg_color[1] + 0.2).min(1.0), (bg_color[2] + 0.2).min(1.0), 1.0,
                ]);
                let _active_token = ui.push_style_color(StyleColor::ButtonActive, [
                    (bg_color[0] + 0.3).min(1.0), (bg_color[1] + 0.3).min(1.0), (bg_color[2] + 0.3).min(1.0), 1.0,
                ]);
                ui.small_button(&button_label);
                let min = ui.item_rect_min();
                let max = [min[0] + ui.item_rect_size()[0], min[1] + ui.item_rect_size()[1]];
                bit_rects.push((abs_bit_pos, min, max));
                if byte_idx == 0 && header_positions.len() < 8 {
                    header_positions.push([(min[0] + max[0]) / 2.0, min[1]]);
                }
                if is_selected {
                    let draw_list = ui.get_window_draw_list();
                    draw_list.add_rect(min, max, [1.0, 1.0, 0.0, 1.0]).thickness(2.0).build();
                }
                if ui.is_item_hovered() {
                    if ui.is_mouse_clicked(imgui::MouseButton::Left) {
                        self.quadrants[idx].selection_start = Some(abs_bit_pos);
                        self.quadrants[idx].selection_end = Some(abs_bit_pos);
                        self.quadrants[idx].is_dragging = true;
                    }
                    let activity_val = self.get_bit_activity_quadrant(idx, abs_bit_pos);
                    let sig_name = signal_name.clone();
                    let dbc_bit = display_pos_to_dbc_bit(abs_bit_pos);
                    ui.tooltip(|| {
                        ui.text(format!("DBC bit {} (byte {}, bit {})", dbc_bit, byte_idx, bit_idx));
                        ui.text(format!("Value: {}", bit_val));
                        if let Some(ref name) = sig_name {
                            ui.separator();
                            ui.text_colored([0.5, 0.8, 1.0, 1.0], format!("Signal: {}", name));
                            if is_msb { ui.text_colored([0.9, 0.9, 0.5, 1.0], "(MSB)"); }
                            if is_lsb { ui.text_colored([0.9, 0.9, 0.5, 1.0], "(LSB)"); }
                        }
                        if activity_val > 0.0 {
                            ui.text_colored([1.0, 0.7, 0.4, 1.0], format!("Activity: {:.0}%", activity_val * 100.0));
                        }
                    });
                }
                if bit_idx > 0 { ui.same_line(); }
            }
            ui.same_line();
            ui.text_colored([0.6, 0.6, 0.6, 1.0], format!("{:02X}", byte_val));
            if byte_idx == 0 && !header_positions.is_empty() {
                let draw_list = ui.get_window_draw_list();
                for (i, pos) in header_positions.iter().enumerate() {
                    let bit = 7 - i;
                    let text = format!("{}", bit);
                    let text_width = ui.calc_text_size(&text)[0];
                    let text_y = pos[1] - ui.text_line_height_with_spacing();
                    draw_list.add_text([pos[0] - text_width / 2.0, text_y], [0.7, 0.7, 0.7, 1.0], text);
                }
            }
        }

        if self.quadrants[idx].is_dragging {
            let mouse_pos = ui.io().mouse_pos;
            for (abs_bit, min, max) in &bit_rects {
                if mouse_pos[0] >= min[0] && mouse_pos[0] <= max[0] && mouse_pos[1] >= min[1] && mouse_pos[1] <= max[1] {
                    self.quadrants[idx].selection_end = Some(*abs_bit);
                    break;
                }
            }
            if ui.is_mouse_released(imgui::MouseButton::Left) {
                let has_selection = self.quadrants[idx].selection_start.is_some() && self.quadrants[idx].selection_end.is_some();
                self.quadrants[idx].is_dragging = false;
                if has_selection {
                    self.open_create_dialog(idx);
                }
            }
        }

        let q = &self.quadrants[idx];
        if let (Some(start), Some(end)) = (q.selection_start, q.selection_end) {
            if !q.is_dragging {
                let (min_disp, max_disp) = if start <= end { (start, end) } else { (end, start) };
                let min_dbc = display_pos_to_dbc_bit(min_disp);
                let max_dbc = display_pos_to_dbc_bit(max_disp);
                let (min_bit, max_bit) = (min_dbc.min(max_dbc), min_dbc.max(max_dbc));
                let bit_count = max_bit - min_bit + 1;
                ui.text_colored([1.0, 1.0, 0.0, 1.0], format!("DBC bits {}-{} ({} bits)", min_bit, max_bit, bit_count));
                ui.same_line();
                if ui.small_button(&format!("Clear##sel{}", idx)) {
                    self.quadrants[idx].selection_start = None;
                    self.quadrants[idx].selection_end = None;
                }
            }
        }
    }

    fn get_bit_activity_quadrant(&self, idx: usize, bit_pos: usize) -> f32 {
        let q = &self.quadrants[idx];
        if q.max_flip_count == 0 { return 0.0; }
        let count = q.bit_flip_counts[bit_pos];
        if count == 0 { 0.0 } else { (count as f32 / q.max_flip_count as f32).sqrt() }
    }

    fn get_selection_bits_quadrant(&self, idx: usize) -> Vec<usize> {
        let q = &self.quadrants[idx];
        match (q.selection_start, q.selection_end) {
            (Some(start), Some(end)) => {
                let (min_bit, max_bit) = if start <= end { (start, end) } else { (end, start) };
                (min_bit..=max_bit).collect()
            }
            _ => Vec::new(),
        }
    }

    fn open_create_dialog(&mut self, quadrant: usize) {
        self.signal_counter += 1;
        self.new_signal_name = format!("NEW_SIGNAL_{}", self.signal_counter);
        self.new_signal_factor = String::from("1");
        self.new_signal_offset = String::from("0");
        self.new_signal_unit = String::new();
        self.show_create_dialog = true;
        self.create_quadrant = Some(quadrant);
    }

    fn open_edit_dialog(&mut self, quadrant: usize, signal_idx: usize, signal: &DbcSignal) {
        self.edit_quadrant = Some(quadrant);
        self.editing_signal_idx = Some(signal_idx);
        self.editing_signal_name = signal.name.clone();
        self.edit_start_bit = signal.start_bit;
        self.edit_bit_length = signal.bit_length;
        self.edit_is_signed = signal.value_type == ValueType::Signed;
        self.edit_is_little_endian = signal.byte_order == ByteOrder::Intel;
        self.edit_factor = signal.factor.to_string();
        self.edit_offset = signal.offset.to_string();
        self.edit_unit = signal.unit.clone().unwrap_or_default();
        self.show_edit_dialog = true;
    }

    fn render_create_dialog(&mut self, ui: &Ui, dbc: &mut DbcFile) {
        if !self.show_create_dialog { return; }
        let quadrant = match self.create_quadrant {
            Some(q) => q,
            None => return,
        };
        let q = &self.quadrants[quadrant];
        let (start_bit, bit_length) = if let (Some(s), Some(e)) = (q.selection_start, q.selection_end) {
            let (min_disp, max_disp) = if s <= e { (s, e) } else { (e, s) };
            let min_dbc = display_pos_to_dbc_bit(min_disp);
            let max_dbc = display_pos_to_dbc_bit(max_disp);
            let start = min_dbc.min(max_dbc);
            let end = min_dbc.max(max_dbc);
            (start as u8, (end - start + 1) as u8)
        } else {
            (0, 1)
        };

        let mut dialog_open = self.show_create_dialog;
        let mut name = self.new_signal_name.clone();
        let mut is_little_endian = self.new_signal_is_little_endian;
        let mut is_signed = self.new_signal_is_signed;
        let mut factor = self.new_signal_factor.clone();
        let mut offset = self.new_signal_offset.clone();
        let mut unit = self.new_signal_unit.clone();

        let mut should_create = false;
        let mut should_cancel = false;

        ui.window("Create Signal")
            .size([400.0, 320.0], Condition::FirstUseEver)
            .position([200.0, 200.0], Condition::FirstUseEver)
            .opened(&mut dialog_open)
            .build(|| {
                ui.text("Create New Signal");
                ui.separator();

                ui.text("Name:"); ui.same_line();
                ui.input_text("##name", &mut name).build();

                ui.text(format!("Start bit: {}  |  Length: {} bits", start_bit, bit_length));
                ui.separator();

                ui.text("Byte order:");
                if ui.selectable_config(format!("Intel (little-endian){}", if is_little_endian { " *" } else { "" }))
                    .selected(is_little_endian).build() { is_little_endian = true; }
                if ui.selectable_config(format!("Motorola (big-endian){}", if !is_little_endian { " *" } else { "" }))
                    .selected(!is_little_endian).build() { is_little_endian = false; }

                ui.separator();

                ui.text("Value type:");
                if ui.selectable_config(format!("Unsigned{}", if !is_signed { " *" } else { "" }))
                    .selected(!is_signed).build() { is_signed = false; }
                if ui.selectable_config(format!("Signed{}", if is_signed { " *" } else { "" }))
                    .selected(is_signed).build() { is_signed = true; }

                ui.separator();

                ui.text("Factor:"); ui.same_line();
                ui.input_text("##factor", &mut factor).build();
                ui.text("Offset:"); ui.same_line();
                ui.input_text("##offset", &mut offset).build();
                ui.text("Unit:"); ui.same_line();
                ui.input_text("##unit", &mut unit).build();

                ui.separator();

                if ui.button("Create") { should_create = true; }
                ui.same_line();
                if ui.button("Cancel") { should_cancel = true; }
            });

        self.new_signal_name = name;
        self.new_signal_is_little_endian = is_little_endian;
        self.new_signal_is_signed = is_signed;
        self.new_signal_factor = factor;
        self.new_signal_offset = offset;
        self.new_signal_unit = unit;

        if should_cancel || !dialog_open {
            if let Some(q) = self.create_quadrant {
                self.quadrants[q].selection_start = None;
                self.quadrants[q].selection_end = None;
            }
            self.show_create_dialog = false;
            self.create_quadrant = None;
        } else if should_create {
            if let Some(msg_id) = self.quadrants[quadrant].selected_message_id {
                if let Ok(factor_val) = self.new_signal_factor.parse::<f64>() {
                    if let Ok(offset_val) = self.new_signal_offset.parse::<f64>() {
                        let signal = DbcSignal {
                            name: self.new_signal_name.clone(),
                            start_bit,
                            bit_length,
                            byte_order: if self.new_signal_is_little_endian { ByteOrder::Intel } else { ByteOrder::Motorola },
                            value_type: if self.new_signal_is_signed { ValueType::Signed } else { ValueType::Unsigned },
                            factor: factor_val,
                            offset: offset_val,
                            minimum: None,
                            maximum: None,
                            unit: if self.new_signal_unit.is_empty() { None } else { Some(self.new_signal_unit.clone()) },
                            multiplexor: None,
                        };

                        if dbc.get_message(msg_id).is_none() {
                            let msg_name = format!("MSG_{:03X}", msg_id);
                            dbc.add_message(DbcMessage::new(msg_id, &msg_name, 8));
                        }

                        if let Some(msg) = dbc.get_message_mut(msg_id) {
                            msg.add_signal(signal.clone());
                        }

                        if let Some(ref mut callback) = *self.on_signal_created.borrow_mut() {
                            callback(msg_id, signal);
                        }
                    }
                }
            }
            if let Some(q) = self.create_quadrant {
                self.quadrants[q].selection_start = None;
                self.quadrants[q].selection_end = None;
            }
            self.show_create_dialog = false;
            self.create_quadrant = None;
        }

        self.show_create_dialog = dialog_open && !should_cancel && !should_create;
    }

    fn render_edit_dialog(&mut self, ui: &Ui, dbc: &mut DbcFile) {
        if !self.show_edit_dialog { return; }

        let mut dialog_open = self.show_edit_dialog;
        let mut name = self.editing_signal_name.clone();
        let mut start_bit = self.edit_start_bit;
        let mut bit_length = self.edit_bit_length;
        let mut is_little_endian = self.edit_is_little_endian;
        let mut is_signed = self.edit_is_signed;
        let mut factor = self.edit_factor.clone();
        let mut offset = self.edit_offset.clone();
        let mut unit = self.edit_unit.clone();

        let mut should_save = false;
        let mut should_cancel = false;
        let mut should_delete = false;

        ui.window("Edit Signal")
            .size([400.0, 380.0], Condition::FirstUseEver)
            .position([200.0, 200.0], Condition::FirstUseEver)
            .opened(&mut dialog_open)
            .build(|| {
                ui.text("Edit Signal");
                ui.separator();

                ui.text("Name:"); ui.same_line();
                ui.input_text("##name", &mut name).build();

                ui.separator();

                // Editable bit position - using input_text and parsing
                ui.text("Start bit:"); ui.same_line();
                let mut start_str = start_bit.to_string();
                ui.input_text("##startbit", &mut start_str).build();
                if let Ok(v) = start_str.parse::<u8>() {
                    start_bit = v.min(63);
                }
                ui.text("Bit length:"); ui.same_line();
                let mut len_str = bit_length.to_string();
                ui.input_text("##bitlen", &mut len_str).build();
                if let Ok(v) = len_str.parse::<u8>() {
                    bit_length = v.max(1).min(64);
                }

                ui.separator();

                ui.text("Byte order:");
                if ui.selectable_config(format!("Intel (little-endian){}", if is_little_endian { " *" } else { "" }))
                    .selected(is_little_endian).build() { is_little_endian = true; }
                if ui.selectable_config(format!("Motorola (big-endian){}", if !is_little_endian { " *" } else { "" }))
                    .selected(!is_little_endian).build() { is_little_endian = false; }

                ui.separator();

                ui.text("Value type:");
                if ui.selectable_config(format!("Unsigned{}", if !is_signed { " *" } else { "" }))
                    .selected(!is_signed).build() { is_signed = false; }
                if ui.selectable_config(format!("Signed{}", if is_signed { " *" } else { "" }))
                    .selected(is_signed).build() { is_signed = true; }

                ui.separator();

                ui.text("Factor:"); ui.same_line();
                ui.input_text("##factor", &mut factor).build();
                ui.text("Offset:"); ui.same_line();
                ui.input_text("##offset", &mut offset).build();
                ui.text("Unit:"); ui.same_line();
                ui.input_text("##unit", &mut unit).build();

                ui.separator();

                if ui.button("Save") { should_save = true; }
                ui.same_line();
                if ui.button("Cancel") { should_cancel = true; }
                ui.same_line();
                ui.text_colored([0.5, 0.5, 0.5, 1.0], "  |  ");
                ui.same_line();
                let _del_color = ui.push_style_color(StyleColor::Button, [0.6, 0.2, 0.2, 1.0]);
                if ui.button("Delete") { should_delete = true; }
            });

        self.editing_signal_name = name;
        self.edit_start_bit = start_bit;
        self.edit_bit_length = bit_length;
        self.edit_is_little_endian = is_little_endian;
        self.edit_is_signed = is_signed;
        self.edit_factor = factor;
        self.edit_offset = offset;
        self.edit_unit = unit;

        if should_cancel || !dialog_open {
            self.show_edit_dialog = false;
            self.edit_quadrant = None;
            self.editing_signal_idx = None;
        } else if should_delete {
            if let Some(quadrant) = self.edit_quadrant {
                if let Some(msg_id) = self.quadrants[quadrant].selected_message_id {
                    if let Some(idx) = self.editing_signal_idx {
                        if let Some(msg) = dbc.get_message_mut(msg_id) {
                            if idx < msg.signals.len() {
                                msg.signals.remove(idx);
                            }
                        }
                    }
                }
            }
            self.show_edit_dialog = false;
            self.edit_quadrant = None;
            self.editing_signal_idx = None;
        } else if should_save {
            if let Some(quadrant) = self.edit_quadrant {
                if let Some(msg_id) = self.quadrants[quadrant].selected_message_id {
                    if let Some(idx) = self.editing_signal_idx {
                        if let Ok(factor_val) = self.edit_factor.parse::<f64>() {
                            if let Ok(offset_val) = self.edit_offset.parse::<f64>() {
                                if let Some(msg) = dbc.get_message_mut(msg_id) {
                                    if idx < msg.signals.len() {
                                        msg.signals[idx].name = self.editing_signal_name.clone();
                                        msg.signals[idx].start_bit = self.edit_start_bit;
                                        msg.signals[idx].bit_length = self.edit_bit_length;
                                        msg.signals[idx].byte_order = if self.edit_is_little_endian { ByteOrder::Intel } else { ByteOrder::Motorola };
                                        msg.signals[idx].value_type = if self.edit_is_signed { ValueType::Signed } else { ValueType::Unsigned };
                                        msg.signals[idx].factor = factor_val;
                                        msg.signals[idx].offset = offset_val;
                                        msg.signals[idx].unit = if self.edit_unit.is_empty() { None } else { Some(self.edit_unit.clone()) };
                                    }
                                }
                            }
                        }
                    }
                }
            }
            self.show_edit_dialog = false;
            self.edit_quadrant = None;
            self.editing_signal_idx = None;
        }

        self.show_edit_dialog = dialog_open && !should_cancel && !should_save && !should_delete;
    }

    fn get_signal_info_quadrant(&self, dbc: &DbcFile, idx: usize) -> Vec<SignalInfo> {
        let mut result = Vec::new();
        let q = &self.quadrants[idx];
        if let Some(id) = q.selected_message_id {
            if let Some(bus) = q.selected_bus {
                if let Some(msg_def) = dbc.get_message(id) {
                    for (i, signal) in msg_def.signals.iter().enumerate() {
                        // Use hash of signal name for consistent color across messages
                        // This ensures the same signal name always gets the same color
                        let color_idx = Self::hash_color_index(&signal.name);
                        result.push(SignalInfo {
                            name: signal.name.clone(),
                            start_bit: signal.start_bit,
                            bit_length: signal.bit_length,
                            byte_order: signal.byte_order,
                            color_idx,
                            bus_id: bus,  // Include bus in signal info
                        });
                    }
                }
            }
        }

        result
    }

    fn get_bit_signal_info(&self, display_pos: usize, signals: &[SignalInfo]) -> ([f32; 4], Option<String>, bool, bool) {
        for signal in signals {
            let display_bits = signal.get_display_positions();
            if display_bits.contains(&display_pos) {
                let color = SIGNAL_COLORS[signal.color_idx];
                let is_msb = display_pos == signal.get_msb_display_pos();
                let is_lsb = display_pos == signal.get_lsb_display_pos();
                return (color, Some(signal.name.clone()), is_msb, is_lsb);
            }
        }
        ([0.15, 0.15, 0.15, 1.0], None, false, false)
    }

    fn render_decoded_signals_quadrant(&mut self, ui: &Ui, dbc: &mut DbcFile, idx: usize) {
        let (id, bus, current_data) = {
            let q = &self.quadrants[idx];
            (
                q.selected_message_id,
                q.selected_bus.unwrap_or(0),
                q.current_data,
            )
        };
        ui.text("Signals:");

        if let Some(id) = id {
            if let Some(msg_def) = dbc.get_message(id) {
                if msg_def.signals.is_empty() {
                    ui.text_colored([0.6, 0.6, 0.6, 1.0], "  No signals defined");
                    ui.text_colored([0.6, 0.6, 0.6, 1.0], "  Click and drag on bits to create one");
                    return;
                }

                // Collect signal data first to avoid borrow issues
                let signal_data: Vec<(String, u8, u8, ByteOrder, ValueType, f64, f64, Option<String>)> =
                    msg_def.signals.iter()
                        .map(|s| (
                            s.name.clone(),
                            s.start_bit,
                            s.bit_length,
                            s.byte_order,
                            s.value_type,
                            s.factor,
                            s.offset,
                            s.unit.clone()
                        ))
                        .collect();

                // Get charted signals for highlighting (clone to avoid borrow issues)
                let charted: Vec<String> = self.charted_signals.borrow().clone();

                // Three columns: Signal name, Value (fixed-width formats, no bounce), Chart button
                let avail_width = ui.content_region_avail()[0];
                let chart_btn_width = 45.0;
                const VALUE_COL_WIDTH: f32 = 115.0;  // Wide enough for " 12345.678 (  123)"
                let signal_col_width = avail_width - chart_btn_width - VALUE_COL_WIDTH - 8.0;

                ui.columns(3, "signal_cols", false);
                ui.set_column_width(0, signal_col_width);
                ui.set_column_width(1, VALUE_COL_WIDTH);
                ui.set_column_width(2, chart_btn_width);

                for (i, (name, start_bit, bit_length, byte_order, value_type, factor, offset, unit)) in signal_data.iter().enumerate() {
                    let color = SIGNAL_COLORS[i % SIGNAL_COLORS.len()];

                    // Column 0: Color swatch + Signal name (clickable for edit)
                    let _color_token = ui.push_style_color(StyleColor::Button, color);
                    ui.small_button(" ");
                    drop(_color_token);
                    ui.same_line();

                    // Signal name - muted color to distinguish from values
                    let _name_color = ui.push_style_color(StyleColor::Text, [0.7, 0.7, 0.75, 1.0]);
                    let is_selected = self.edit_quadrant == Some(idx) && self.editing_signal_idx == Some(i);
                    let signal = DbcSignal {
                        name: name.clone(),
                        start_bit: *start_bit,
                        bit_length: *bit_length,
                        byte_order: *byte_order,
                        value_type: *value_type,
                        factor: *factor,
                        offset: *offset,
                        unit: unit.clone(),
                        minimum: None,
                        maximum: None,
                        multiplexor: None,
                    };
                    if ui.selectable_config(&format!("{}##q{}s{}", name, idx, i)).selected(is_selected).build() {
                        self.open_edit_dialog(idx, i, &signal);
                    }
                    drop(_name_color);

                    if ui.is_item_hovered() {
                        ui.tooltip(|| {
                            ui.text_colored([0.7, 0.7, 0.7, 1.0], "Click to edit");
                        });
                    }

                    ui.next_column();

                    // Column 1: Decoded value - fixed width, left-aligned, clipped to prevent overlap
                    let (value_str, raw_str): (String, Option<String>) = if let Some(raw_value) = extract_bits(
                        &current_data,
                        *start_bit,
                        *bit_length,
                        *byte_order
                    ) {
                        let raw_value_i64 = if *value_type == ValueType::Signed {
                            sign_extend(raw_value, *bit_length)
                        } else {
                            raw_value as i64
                        };

                        let value_desc = dbc.value_tables.get(name)
                            .and_then(|descriptions| {
                                descriptions.iter()
                                    .find(|d| d.value == raw_value_i64)
                                    .map(|d| d.description.clone())
                            });

                        // Fixed-width formats: value and raw never change character count = no bounce
                        let raw_fmt = format!("({:>6})", raw_value_i64);
                        if let Some(desc) = value_desc {
                            // Enum: pad to 10 chars
                            (format!("{:>10}", desc), Some(raw_fmt))
                        } else {
                            let physical_value = (raw_value_i64 as f64) * factor + offset;
                            // Numeric: pad to 10.3 + 4 for unit = fixed width
                            let s = if let Some(ref u) = unit {
                                if u.is_empty() {
                                    format!("{:>12.3}", physical_value)
                                } else {
                                    format!("{:>10.3} {:>4}", physical_value, u)
                                }
                            } else {
                                format!("{:>12.3}", physical_value)
                            };
                            (s, Some(raw_fmt))
                        }
                    } else {
                        ("—".to_string(), None)
                    };

                    // Draw value + raw directly in column (no child window - was causing overlap)
                    ui.text_colored([0.45, 0.9, 1.0, 1.0], &value_str);
                    if let Some(ref r) = raw_str {
                        ui.same_line();
                        ui.text_colored([0.5, 0.5, 0.55, 1.0], r);
                    }

                    ui.next_column();

                    // Column 2: Chart button
                    let is_charted = self.is_signal_charted(name, bus);
                    let btn_color = if is_charted {
                        [0.2, 0.6, 0.3, 0.9]  // Green if charted
                    } else {
                        [0.3, 0.3, 0.4, 0.8]  // Gray if not
                    };

                    let _chart_color = ui.push_style_color(StyleColor::Button, btn_color);
                    // Use simple ASCII characters that render everywhere
                    let btn_label = if is_charted { "+" } else { "+" };
                    if ui.small_button(&format!("{}##chart{}q{}", btn_label, i, idx)) {
                        self.request_chart_toggle(name.clone(), bus);
                    }
                    drop(_chart_color);

                    if ui.is_item_hovered() {
                        ui.tooltip(|| {
                            if is_charted {
                                ui.text("Remove from chart");
                            } else {
                                ui.text("Add to chart");
                            }
                        });
                    }

                    ui.next_column();
                }

                ui.columns(1, "", false);
            } else {
                ui.text_colored([0.6, 0.6, 0.6, 1.0], "  Not in DBC");
            }
        } else {
            ui.text_colored([0.6, 0.6, 0.6, 1.0], "  No message");
        }
    }

    /// Generate a consistent color index for a signal name using a simple hash
    /// This ensures the same signal name always gets the same color
    fn hash_color_index(name: &str) -> usize {
        let mut hash: usize = 5381;
        for c in name.bytes() {
            hash = hash.wrapping_mul(33).wrapping_add(c as usize);
        }
        hash % SIGNAL_COLORS.len()
    }
}

fn sign_extend(value: u64, bit_length: u8) -> i64 {
    if bit_length >= 64 { return value as i64; }
    let sign_bit = 1u64 << (bit_length - 1);
    if value & sign_bit != 0 {
        let mask = !((1u64 << bit_length) - 1);
        (value | mask) as i64
    } else {
        value as i64
    }
}

impl Default for BitVisualizerWindow {
    fn default() -> Self {
        Self::new()
    }
}

struct SignalInfo {
    name: String,
    start_bit: u8,
    bit_length: u8,
    byte_order: ByteOrder,
    color_idx: usize,
    bus_id: u8,
}

/// Convert DBC bit position to display grid position.
/// DBC uses LSB-first: bit 0 = LSB (rightmost), bit 7 = MSB (leftmost).
/// Display uses MSB-first: position 0 = leftmost (MSB), position 7 = rightmost (LSB).
fn dbc_bit_to_display_pos(dbc_bit: usize) -> usize {
    (dbc_bit / 8) * 8 + (7 - (dbc_bit % 8))
}

/// Convert display grid position to DBC bit position.
fn display_pos_to_dbc_bit(display_pos: usize) -> usize {
    (display_pos / 8) * 8 + (7 - (display_pos % 8))
}

impl SignalInfo {
    /// DBC bit positions (0=LSB, 7=MSB within byte 0)
    fn get_dbc_bit_positions(&self) -> Vec<usize> {
        let start = self.start_bit as usize;
        let end = start + self.bit_length as usize;
        (start..end).collect()
    }

    /// Display grid positions for highlighting (0=leftmost/MSB, 7=rightmost/LSB within byte 0)
    fn get_display_positions(&self) -> Vec<usize> {
        self.get_dbc_bit_positions()
            .into_iter()
            .map(dbc_bit_to_display_pos)
            .collect()
    }

    fn get_msb_display_pos(&self) -> usize {
        let dbc_msb = match self.byte_order {
            ByteOrder::Intel => self.start_bit as usize + self.bit_length as usize - 1,
            ByteOrder::Motorola => self.start_bit as usize,
        };
        dbc_bit_to_display_pos(dbc_msb)
    }

    fn get_lsb_display_pos(&self) -> usize {
        let dbc_lsb = match self.byte_order {
            ByteOrder::Intel => self.start_bit as usize,
            ByteOrder::Motorola => self.start_bit as usize + self.bit_length as usize - 1,
        };
        dbc_bit_to_display_pos(dbc_lsb)
    }
}
