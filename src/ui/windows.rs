use imgui::{Condition, StyleColor, Ui};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use crate::core::CanMessage;
use crate::core::dbc::DbcFile;

/// Direction: RX (received) or TX (sent)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MessageDirection {
    Rx,
    Tx,
}

/// State tracking for a single CAN message ID on a specific bus
#[derive(Clone, Debug)]
pub struct MessageState {
    pub id: u32,
    pub bus: u8,
    pub direction: MessageDirection,
    pub name: String,
    pub data: Vec<u8>,
    pub byte_colors: Vec<[f32; 4]>,
    pub count: u32,
    pub freq: f32,
    pub last_timestamp: f64,
    pub last_update: Instant,
    // For frequency calculation
    freq_samples: Vec<f64>,
}

impl MessageState {
    pub fn new(id: u32, bus: u8, direction: MessageDirection) -> Self {
        let suffix = match direction {
            MessageDirection::Rx => "",
            MessageDirection::Tx => " (TX)",
        };
        Self {
            id,
            bus,
            direction,
            name: format!("MSG_0x{:03X}{}", id, suffix),
            data: Vec::new(),
            byte_colors: Vec::new(),
            count: 0,
            freq: 0.0,
            last_timestamp: 0.0,
            last_update: Instant::now(),
            freq_samples: Vec::with_capacity(10),
        }
    }

    pub fn update(&mut self, msg: &CanMessage, msg_name: Option<&str>) {
        // Update name if provided (DBC names override default names)
        if let Some(name) = msg_name {
            if !name.is_empty() {
                self.name = name.to_string();
            }
        }

        // Calculate frequency
        if self.count > 0 && msg.timestamp_unix() > self.last_timestamp {
            let delta = msg.timestamp_unix() - self.last_timestamp;
            if delta > 0.0 {
                self.freq_samples.push(1.0 / delta);
                if self.freq_samples.len() > 10 {
                    self.freq_samples.remove(0);
                }
                // Moving average
                self.freq = self.freq_samples.iter().sum::<f64>() as f32 / self.freq_samples.len() as f32;
            }
        }

        // Update data and calculate colors
        let old_data = self.data.clone();
        self.data = msg.data.clone();
        self.byte_colors = self.calculate_byte_colors(&old_data, &msg.data);

        self.count += 1;
        self.last_timestamp = msg.timestamp_unix();
        self.last_update = Instant::now();
    }

    fn calculate_byte_colors(&self, old_data: &[u8], new_data: &[u8]) -> Vec<[f32; 4]> {
        let mut colors = Vec::with_capacity(new_data.len());

        for (i, &new_byte) in new_data.iter().enumerate() {
            let old_byte = old_data.get(i).copied().unwrap_or(0);
            let diff = new_byte ^ old_byte;

            let color = if self.count == 0 {
                // First message - no change yet
                [0.3, 0.3, 0.35, 1.0]
            } else if diff == 0 {
                // No change
                [0.25, 0.25, 0.28, 1.0]
            } else {
                // Changed - color based on pattern
                let change_ratio = (diff.count_ones() as f32) / 8.0;

                if diff == 0xFF {
                    // All bits changed (toggle?)
                    [0.9, 0.6, 0.2, 1.0] // Orange
                } else if new_byte > old_byte {
                    // Increasing
                    [0.3, 0.7, 0.4, 1.0] // Green
                } else if new_byte < old_byte {
                    // Decreasing
                    [0.7, 0.4, 0.3, 1.0] // Red
                } else {
                    // Mixed change
                    [0.5, 0.5, 0.2 + change_ratio * 0.5, 1.0] // Yellow-ish
                }
            };
            colors.push(color);
        }

        colors
    }

    pub fn hex_data(&self) -> String {
        self.data.iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn freq_str(&self) -> String {
        if self.freq >= 1000.0 {
            format!("{:.1} kHz", self.freq / 1000.0)
        } else if self.freq >= 1.0 {
            format!("{:.1} Hz", self.freq)
        } else {
            format!("{:.0} mHz", self.freq * 1000.0)
        }
    }

    pub fn is_active(&self) -> bool {
        self.last_update.elapsed() < Duration::from_millis(500)
    }
}

/// Key: (CAN ID, bus, direction)
type MessageKey = (u32, u8, MessageDirection);

/// Window showing live CAN message state - one row per CAN ID + direction (Cabana style)
pub struct MessageListWindow {
    /// Map of (CAN ID, bus, direction) to current state
    states: HashMap<MessageKey, MessageState>,
    /// All messages (for full history mode)
    messages: Vec<CanMessage>,
    /// Selected (CAN ID, bus, direction)
    selected: Option<MessageKey>,
    /// Display mode
    live_mode: bool,
    /// Filter string
    filter: String,
    /// Sort column
    sort_column: usize,
    sort_ascending: bool,
    /// DBC file for message names
    dbc_file: Option<DbcFile>,
}

impl MessageListWindow {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
            messages: Vec::new(),
            selected: None,
            live_mode: true,
            filter: String::new(),
            sort_column: 0,
            sort_ascending: true,
            dbc_file: None,
        }
    }

    pub fn set_messages(&mut self, messages: Vec<CanMessage>) {
        self.messages = messages;
    }

    /// Append messages (for streaming load)
    pub fn append_messages(&mut self, msgs: &[CanMessage]) {
        self.messages.extend_from_slice(msgs);
    }

    pub fn set_dbc(&mut self, dbc: DbcFile) {
        self.dbc_file = Some(dbc);

        // Update all existing message names with DBC names
        if let Some(ref dbc) = self.dbc_file {
            for ((msg_id, _bus, _dir), state) in self.states.iter_mut() {
                if let Some(msg_def) = dbc.get_message(*msg_id) {
                    let suffix = match state.direction {
                        MessageDirection::Rx => "",
                        MessageDirection::Tx => " (TX)",
                    };
                    state.name = format!("{}{}", msg_def.name, suffix);
                }
            }
        }
    }

    /// Update state with a received message (called during playback/live RX)
    pub fn update_message(&mut self, msg: &CanMessage) {
        self.update_message_with_direction(msg, MessageDirection::Rx);
    }

    /// Update state with a sent message (called when plugins send)
    pub fn add_sent_message(&mut self, msg: &CanMessage) {
        self.update_message_with_direction(msg, MessageDirection::Tx);
    }

    fn update_message_with_direction(&mut self, msg: &CanMessage, direction: MessageDirection) {
        let key = (msg.id, msg.bus, direction);
        let state = self.states.entry(key).or_insert_with(|| MessageState::new(msg.id, msg.bus, direction));

        // Get message name from DBC if available (RX only - TX keeps suffix)
        let msg_name = self.dbc_file.as_ref()
            .and_then(|dbc| dbc.get_message(msg.id))
            .map(|m| {
                let base = m.name.as_str();
                match direction {
                    MessageDirection::Rx => base.to_string(),
                    MessageDirection::Tx => format!("{} (TX)", base),
                }
            });

        state.update(msg, msg_name.as_deref());
    }

    /// Clear all states
    pub fn clear(&mut self) {
        self.states.clear();
        self.messages.clear();
        self.selected = None;
    }

    pub fn selected_message(&self) -> Option<&MessageState> {
        self.selected.and_then(|key| self.states.get(&key))
    }

    /// Get latest state for a message by (id, bus) - prefers RX, used for bit visualizer
    pub fn get_state(&self, id: u32, bus: u8) -> Option<&MessageState> {
        self.states.get(&(id, bus, MessageDirection::Rx))
            .or_else(|| self.states.get(&(id, bus, MessageDirection::Tx)))
    }

    /// Debug info
    pub fn debug_info(&self) -> (Option<(u32, u8)>, usize, usize) {
        (self.selected.map(|(id, bus, _)| (id, bus)), self.states.len(), self.messages.len())
    }

    pub fn render(&mut self, ui: &Ui, is_open: &mut bool, is_playing: bool) {
        ui.window("Messages")
            .size([500.0, 400.0], Condition::FirstUseEver)
            .position([10.0, 30.0], Condition::FirstUseEver)
            .opened(is_open)
            .build(|| {
                self.render_content(ui, is_playing);
            });
    }

    pub fn render_content(&mut self, ui: &Ui, is_playing: bool) {
        // Mode toggle
        let mut mode = if self.live_mode { 0 } else { 1 };
        if ui.radio_button("Live", &mut mode, 0) {
            self.live_mode = mode == 0;
        }
        ui.same_line();
        if ui.radio_button("History", &mut mode, 1) {
            self.live_mode = mode == 0;
        }

        ui.same_line();
        ui.spacing();
        ui.same_line();

        if ui.small_button("Clear") {
            self.clear();
        }

        ui.same_line();

        // Filter
        ui.text("Filter:");
        ui.same_line();
        let _ = ui.input_text("##filter", &mut self.filter)
            .hint("ID or name...")
            .build();

        ui.separator();

        if self.live_mode {
            self.render_live_mode(ui, is_playing);
        } else {
            self.render_history_mode(ui);
        }
    }

    fn render_live_mode(&mut self, ui: &Ui, is_playing: bool) {
        // Header
        ui.text("ID   Bus   Dir  Name              Freq     Count   Data");
        ui.separator();

        // Collect and sort states
        let filter_lower = self.filter.to_lowercase();
        let mut sorted_keys: Vec<MessageKey> = self.states.keys().cloned().collect();

        // Apply filter
        if !filter_lower.is_empty() {
            sorted_keys.retain(|key| {
                if let Some(state) = self.states.get(key) {
                    let id_str = format!("0x{:03X}", state.id);
                    let bus_str = format!("{}", state.bus);
                    let name_lower = state.name.to_lowercase();
                    id_str.to_lowercase().contains(&filter_lower) ||
                    bus_str.contains(&filter_lower) ||
                    name_lower.contains(&filter_lower)
                } else {
                    false
                }
            });
        }

        // Sort - use stable sort (ID only) during playback so list doesn't jump when freq/count update
        let effective_sort_col = if is_playing { 0 } else { self.sort_column };
        sorted_keys.sort_by(|&(id_a, bus_a, dir_a), &(id_b, bus_b, dir_b)| {
            let state_a = self.states.get(&(id_a, bus_a, dir_a)).unwrap();
            let state_b = self.states.get(&(id_b, bus_b, dir_b)).unwrap();
            let cmp = match effective_sort_col {
                0 => id_a.cmp(&id_b).then_with(|| dir_a.cmp(&dir_b)),
                1 => bus_a.cmp(&bus_b).then_with(|| dir_a.cmp(&dir_b)),
                2 => state_a.name.cmp(&state_b.name),
                3 => state_a.freq.partial_cmp(&state_b.freq).unwrap_or(std::cmp::Ordering::Equal),
                4 => state_a.count.cmp(&state_b.count),
                _ => id_a.cmp(&id_b),
            };
            if self.sort_ascending { cmp } else { cmp.reverse() }
        });

        // Render rows with two columns: ID|Bus|Dir|Name|Freq|Count | Data (colored bytes)
        ui.columns(2, "msg_list_cols", false);
        ui.set_column_width(0, 360.0);  // Wide enough for ID, Bus, Dir, Name (18), Freq (8), Count (6)

        for key in sorted_keys {
            let (id, bus, dir) = key;
            let state = self.states.get(&key).unwrap();
            let is_selected = self.selected == Some(key);

            // TX rows: blue-tinted text to distinguish from RX
            let dir_str = match dir {
                MessageDirection::Rx => "RX",
                MessageDirection::Tx => "TX",
            };
            let _tx_color = match dir {
                MessageDirection::Rx => None,
                MessageDirection::Tx => Some(ui.push_style_color(StyleColor::Text, [0.4, 0.7, 1.0, 1.0])),
            };

            // Column 0: ID, Bus, Dir, Name, Freq, Count
            let name_padded = format!("{:<18}", &state.name[..state.name.len().min(18)]);
            let row_label = format!("0x{:03X}  {}    {}  {}{:>8}  {:>6}",
                id, bus, dir_str, name_padded, state.freq_str(), state.count);

            // Stable ID + span full row: during rapid playback, (1) label must not change or
            // ImGui loses the click, (2) full row must be clickable (including colored bytes).
            let id_scope = ui.push_id(&format!("msg_{}_{}_{:?}", id, bus, dir));
            let clicked = ui.selectable_config("##row")
                .selected(is_selected)
                .span_all_columns(true)
                .build();
            if clicked {
                self.selected = Some(key);
            }
            // Draw display text over the selectable (text is non-interactive, can change every frame)
            ui.same_line_with_spacing(0.0, 0.0);
            ui.text(&row_label);
            id_scope.pop();

            if ui.is_item_hovered() {
                ui.tooltip(|| {
                    ui.text(format!("Data: {}", state.hex_data()));
                });
            }

            // Column 1: Colored bytes
            ui.next_column();
            self.render_colored_bytes(ui, state);
            ui.next_column();
        }

        ui.columns(1, "", false);

        // Show selected message details
        if let Some(state) = self.selected_message() {
            ui.separator();
            self.render_message_details(ui, state);
        }
    }

    fn render_colored_bytes(&self, ui: &Ui, state: &MessageState) {
        let draw_list = ui.get_window_draw_list();
        let cursor = ui.cursor_screen_pos();

        let byte_width = 22.0;
        let byte_height = 18.0;
        let gap = 2.0;

        for (i, (&byte, &color)) in state.data.iter().zip(state.byte_colors.iter()).enumerate() {
            // Add gap every 4 bytes
            let gap_offset = (i / 4) as f32 * 4.0;

            let x = cursor[0] + (i as f32 * byte_width) + gap_offset;
            let y = cursor[1];

            // Background color
            draw_list.add_rect(
                [x, y],
                [x + byte_width - gap, y + byte_height],
                color,
            ).filled(true).rounding(2.0).build();

            // Hex text
            let hex = format!("{:02X}", byte);
            let text_color = if color[0] + color[1] + color[2] > 1.5 {
                [0.0, 0.0, 0.0, 1.0]
            } else {
                [1.0, 1.0, 1.0, 1.0]
            };
            draw_list.add_text([x + 3.0, y + 2.0], text_color, hex);
        }

        // Reserve space
        let total_width = (state.data.len() as f32 * byte_width) + ((state.data.len() / 4) as f32 * 4.0);
        ui.dummy([total_width.max(100.0), byte_height]);
    }

    fn render_message_details(&self, ui: &Ui, state: &MessageState) {
        ui.text(format!("Message: {} (0x{:03X})", state.name, state.id));
        ui.text(format!("Frequency: {}", state.freq_str()));
        ui.text(format!("Count: {}", state.count));

        ui.separator();
        ui.text("Data bytes:");

        // Show detailed byte view
        ui.indent();
        for (i, (&byte, &color)) in state.data.iter().zip(state.byte_colors.iter()).enumerate() {
            ui.text_colored(color, format!("[{:2}] {:02X} ({:3})", i, byte, byte));
        }
        ui.unindent();
    }

    fn render_history_mode(&mut self, ui: &Ui) {
        ui.text_wrapped("History mode shows all recorded messages.");
        ui.text(format!("Total messages: {}", self.messages.len()));

        let mut clipper = imgui::ListClipper::new(self.messages.len() as i32).begin(ui);

        while clipper.step() {
            for i in clipper.display_start()..clipper.display_end() {
                let i = i as usize;
                if let Some(msg) = self.messages.get(i) {
                    let label = format!(
                        "{} | 0x{:03X} [Bus {}] | {}",
                        msg.timestamp.format("%H:%M:%S%.3f"),
                        msg.id,
                        msg.bus,
                        msg.hex_data()
                    );

                    if ui.selectable(&label) {
                        eprintln!("MessageList[History]: CLICKED id=0x{:03X}, bus={}", msg.id, msg.bus);
                        self.selected = Some((msg.id, msg.bus, MessageDirection::Rx));
                    }
                }
            }
        }
    }
}

/// Window for editing DBC file definitions
pub struct DbcEditorWindow {
    dbc_file: DbcFile,
    selected_message: Option<u32>,
    /// Pending load request
    pub load_requested: bool,
    /// Pending save request
    pub save_requested: bool,
}

impl DbcEditorWindow {
    pub fn new() -> Self {
        Self {
            dbc_file: DbcFile::new(),
            selected_message: None,
            load_requested: false,
            save_requested: false,
        }
    }

    pub fn set_dbc(&mut self, dbc_file: DbcFile) {
        self.dbc_file = dbc_file;
    }

    pub fn get_dbc(&self) -> &DbcFile {
        &self.dbc_file
    }

    pub fn render(&mut self, ui: &Ui, is_open: &mut bool) {
        ui.window("DBC Editor")
            .size([400.0, 400.0], Condition::FirstUseEver)
            .position([10.0, 450.0], Condition::FirstUseEver)
            .opened(is_open)
            .build(|| {
                self.render_content(ui);
            });
    }

    pub fn render_content(&mut self, ui: &Ui) {
        ui.text("DBC File Editor");
        ui.text("Load a .dbc file to edit signal definitions");

        ui.separator();

        if ui.button("Load DBC") {
            self.load_requested = true;
        }

        ui.same_line();
        if ui.button("Save DBC") {
            self.save_requested = true;
        }

        ui.separator();

        ui.text(format!("Messages: {} defined", self.dbc_file.messages.len()));

        for msg in &self.dbc_file.messages {
            let is_selected = self.selected_message == Some(msg.id);
            let label = format!("0x{:03X} - {} ({})", msg.id, msg.name, msg.size);

            let _token = if is_selected {
                Some(ui.push_style_color(StyleColor::Header, [0.3, 0.3, 0.4, 1.0]))
            } else {
                None
            };

            if ui.selectable(&label) {
                self.selected_message = Some(msg.id);
            }

            drop(_token);
        }

        ui.separator();

        // Show selected message details
        if let Some(msg_id) = self.selected_message {
            if let Some(msg) = self.dbc_file.get_message(msg_id) {
                ui.text(format!("Message: {}", msg.name));
                ui.text(format!("  ID: 0x{:03X}", msg.id));
                ui.text(format!("  Size: {} bytes", msg.size));
                ui.text(format!("  Signals: {}", msg.signals.len()));

                ui.separator();

                ui.text("Signals:");
                for signal in &msg.signals {
                    ui.text(format!("  - {}", signal.name));
                    ui.text(format!(
                        "    Start bit: {}, Length: {}",
                        signal.start_bit, signal.bit_length
                    ));
                    ui.text(format!(
                        "    Factor: {}, Offset: {}",
                        signal.factor, signal.offset
                    ));
                    if let Some(ref unit) = signal.unit {
                        ui.text(format!("    Unit: {}", unit));
                    }
                }
            }
        }
    }
}
