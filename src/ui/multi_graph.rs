use imgui::{StyleColor, Ui, MouseButton};
use chrono::{DateTime, Utc, Duration};
use std::collections::{HashMap, HashSet};

/// A single data series for plotting
#[derive(Clone)]
pub struct DataSeries {
    pub name: String,
    pub msg_id: u32,
    pub bus: u8,
    pub data_points: Vec<(f64, DateTime<Utc>)>,
    pub color: [f32; 4],
    pub visible: bool,
    max_points: usize,
}

impl DataSeries {
    pub fn new(name: String, msg_id: u32, bus: u8, color: [f32; 4]) -> Self {
        Self {
            name,
            msg_id,
            bus,
            data_points: Vec::new(),
            color,
            visible: true,
            max_points: 200000,  // Increased to handle large datasets
        }
    }

    pub fn add_point(&mut self, value: f64, timestamp: DateTime<Utc>) {
        self.data_points.push((value, timestamp));

        // Trim if we exceed max_points, but keep some buffer
        if self.data_points.len() > self.max_points {
            let trim_count = (self.data_points.len() - self.max_points).max(1);
            self.data_points.drain(0..trim_count);
        }
    }

    pub fn clear(&mut self) {
        self.data_points.clear();
    }

    pub fn get_value_range_in_window(&self, time_start: DateTime<Utc>, time_end: DateTime<Utc>) -> (f64, f64) {
        let values: Vec<f64> = self.data_points.iter()
            .filter(|(_, ts)| *ts >= time_start && *ts <= time_end)
            .map(|(v, _)| *v)
            .collect();

        if values.is_empty() {
            return (0.0, 1.0);
        }

        values.iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), &v| {
                (min.min(v), max.max(v))
            })
    }

    pub fn current_value(&self) -> Option<f64> {
        self.data_points.last().map(|(v, _)| *v)
    }
}

/// Signal information for the picker
#[derive(Clone)]
pub struct SignalInfo {
    pub name: String,
    pub msg_id: u32,
    pub bus: u8,
    pub msg_name: String,
    pub unit: String,
}

impl SignalInfo {
    /// Get the display name including bus information
    pub fn display_name(&self) -> String {
        format!("{} [Bus {}]", self.name, self.bus)
    }

    /// Get the unique key for this signal (name + bus)
    pub fn key(&self) -> String {
        format!("{}@bus{}", self.name, self.bus)
    }
}

/// Timeline actions emitted by the chart widget
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TimelineAction {
    None,
    Play,
    Pause,
    StepForward,
    StepBack,
}

/// Charts panel with signal picker - Cabana-style
pub struct MultiSignalGraph {
    series: HashMap<String, DataSeries>,  // Key: "signal_name@busN"
    available_signals: Vec<SignalInfo>,
    show_legend: bool,
    shared_y_axis: bool,
    time_window_secs: f32,
    graph_height: f32,
    show_signal_picker: bool,
    signal_filter: String,
    selected_signals: HashSet<String>,  // Keys: "signal_name@busN"
    /// Pending seek request (offset in seconds from current time)
    seek_request: Option<f32>,
    /// Track if zoom slider is being dragged
    slider_dragging: bool,
    /// Track if timeline slider is being dragged
    timeline_dragging: bool,
    /// Pending timeline action
    timeline_action: Option<TimelineAction>,
    /// Overall data time range (independent of charted signals)
    data_start_time: Option<DateTime<Utc>>,
    data_end_time: Option<DateTime<Utc>>,
}

impl MultiSignalGraph {
    pub fn new() -> Self {
        Self {
            series: HashMap::new(),
            available_signals: Vec::new(),
            show_legend: true,
            shared_y_axis: false,
            time_window_secs: 5.0,
            graph_height: 200.0,
            show_signal_picker: false,
            signal_filter: String::new(),
            selected_signals: HashSet::new(),
            seek_request: None,
            slider_dragging: false,
            timeline_dragging: false,
            timeline_action: None,
            data_start_time: None,
            data_end_time: None,
        }
    }

    /// Take and clear any pending seek request
    pub fn take_seek_request(&mut self) -> Option<f32> {
        self.seek_request.take()
    }

    /// Take and clear any pending timeline action
    pub fn take_timeline_action(&mut self) -> Option<TimelineAction> {
        self.timeline_action.take()
    }

    /// Set available signals from DBC
    pub fn set_available_signals(&mut self, signals: Vec<SignalInfo>) {
        self.available_signals = signals;
    }

    /// Set the overall data time range (independent of charted signals)
    pub fn set_data_time_range(&mut self, start: DateTime<Utc>, end: DateTime<Utc>) {
        self.data_start_time = Some(start);
        self.data_end_time = Some(end);
    }

    /// Clear the data time range
    pub fn clear_time_range(&mut self) {
        self.data_start_time = None;
        self.data_end_time = None;
    }

    /// Check if a signal is charted
    pub fn has_signal(&self, key: &str) -> bool {
        self.series.contains_key(key)
    }

    /// Get list of charted signal names
    pub fn get_charted_signals(&self) -> Vec<String> {
        self.series.keys().cloned().collect()
    }

    /// Toggle a signal on/off the chart by key (name@busN format)
    pub fn toggle_signal_by_name(&mut self, key: &str) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/can-viz-chart-debug.txt")
            .ok();
        if let Some(ref mut f) = f {
            let _ = writeln!(f, "toggle_signal_by_name called with: {}", key);
        }

        if self.series.contains_key(key) {
            if let Some(ref mut f) = f { let _ = writeln!(f, "  signal already in series, removing"); }
            self.series.remove(key);
        } else {
            // Find the signal info by parsing the key to extract name and bus
            if let Some(pos) = key.find("@bus") {
                let name = &key[..pos];
                let bus_str = &key[pos + 4..];
                if let Ok(bus) = bus_str.parse::<u8>() {
                    // Match by name only (DBC definitions are bus-agnostic)
                    // then create a new SignalInfo with the requested bus
                    if let Some(template) = self.available_signals.iter()
                        .find(|s| s.name == name)
                    {
                        let mut info = template.clone();
                        info.bus = bus;  // Use the bus from the request key
                        self.add_signal(&info);
                    }
                }
            }
        }
    }

    /// Add a signal to the chart
    pub fn add_signal(&mut self, info: &SignalInfo) {
        let key = info.key();
        if self.series.contains_key(&key) {
            return;
        }

        let color = self.generate_color(self.series.len());
        let series = DataSeries::new(info.name.clone(), info.msg_id, info.bus, color);
        self.series.insert(key.clone(), series);
        self.selected_signals.insert(key);
    }

    /// Remove a signal from the chart by key
    pub fn remove_signal(&mut self, key: &str) {
        self.series.remove(key);
        self.selected_signals.remove(key);
    }

    /// Restore chart signals from savestate (keys like "signal@bus0")
    pub fn restore_signals(&mut self, keys: &[String]) {
        for key in keys {
            if self.series.contains_key(key) {
                continue;
            }
            if let Some(pos) = key.find("@bus") {
                let name = &key[..pos];
                let bus_str = &key[pos + 4..];
                if let Ok(bus) = bus_str.parse::<u8>() {
                    if let Some(template) = self.available_signals.iter().find(|s| s.name == name) {
                        let mut info = template.clone();
                        info.bus = bus;
                        self.add_signal(&info);
                    }
                }
            }
        }
    }

    /// Add a data point to a series
    pub fn add_point(&mut self, key: &str, value: f64, timestamp: DateTime<Utc>) {
        if let Some(series) = self.series.get_mut(key) {
            series.add_point(value, timestamp);
        }
    }

    /// Clear all data (keep signals, just clear values)
    pub fn clear_data(&mut self) {
        for series in self.series.values_mut() {
            series.clear();
        }
    }

    /// Clear everything including signals
    pub fn clear(&mut self) {
        self.series.clear();
        self.selected_signals.clear();
    }

    /// Generate a distinct color for a series based on index
    fn generate_color(&self, index: usize) -> [f32; 4] {
        let colors = [
            [0.0, 0.75, 1.0, 1.0],
            [1.0, 0.4, 0.4, 1.0],
            [0.4, 1.0, 0.4, 1.0],
            [1.0, 1.0, 0.4, 1.0],
            [1.0, 0.4, 1.0, 1.0],
            [0.4, 1.0, 1.0, 1.0],
            [1.0, 0.6, 0.2, 1.0],
            [0.6, 0.4, 1.0, 1.0],
        ];
        colors[index % colors.len()]
    }

    /// Get list of charted signal names
    pub fn charted_signals(&self) -> Vec<&str> {
        self.series.keys().map(|s| s.as_str()).collect()
    }

    /// Render the charts panel
    /// Shows a sliding time window around current_time.
    pub fn render(&mut self, ui: &Ui, current_time: Option<DateTime<Utc>>, _is_playing: bool) {
        // Toolbar row 1: Add Signal, Clear All, Shared Y, Playback controls
        if ui.small_button("+ Add Signal") {
            self.show_signal_picker = !self.show_signal_picker;
        }
        ui.same_line();
        if ui.small_button("Clear All") {
            self.clear();
        }
        ui.same_line();
        ui.checkbox("Shared Y", &mut self.shared_y_axis);
        ui.same_line();
        ui.text("    ");  // spacing
        ui.same_line();
        if ui.small_button("<<") {
            self.timeline_action = Some(TimelineAction::StepBack);
        }
        ui.same_line();
        if ui.small_button(if _is_playing { "||" } else { ">" }) {
            self.timeline_action = Some(if _is_playing { TimelineAction::Pause } else { TimelineAction::Play });
        }
        ui.same_line();
        if ui.small_button(">>") {
            self.timeline_action = Some(TimelineAction::StepForward);
        }

        ui.spacing();

        // Timeline scrubber (full width) - using overall data time range
        if let (Some(data_start), Some(data_end)) = (self.data_start_time, self.data_end_time) {
            let total_duration_secs = (data_end - data_start).num_seconds() as f32;
            let total_duration_secs = total_duration_secs.max(5.0);

            if let Some(ct) = current_time {
                let current_offset = (ct - data_start).num_seconds() as f32;
                let timeline_pos = (current_offset / total_duration_secs).clamp(0.0, 1.0);

                let slider_width = ui.content_region_avail()[0];

                if let Some(new_pos) = self.timeline_slider_widget(ui, "##timeline_slider", timeline_pos, total_duration_secs, slider_width) {
                    // Handle timeline scrubbing - use RELATIVE seek like the chart does
                    let new_offset = new_pos * total_duration_secs;
                    let target_time = data_start + Duration::seconds(new_offset as i64);
                    // Positive value = relative offset from current time
                    let seek_offset_secs = (target_time - ct).num_milliseconds() as f32 / 1000.0;
                    self.seek_request = Some(seek_offset_secs);
                }
            }
        }

        ui.spacing();

        // Zoom slider (full width)
        let recording_duration_secs = self.series.values()
            .flat_map(|s| s.data_points.iter().map(|(_, ts)| *ts))
            .max()
            .map(|last| {
                self.series.values()
                    .flat_map(|s| s.data_points.iter().map(|(_, ts)| *ts))
                    .min()
                    .map(|first| (last - first).num_seconds() as f32)
                    .unwrap_or(60.0)
            })
            .unwrap_or(60.0)
            .max(5.0); // Minimum 5 second recording

        let slider_width = ui.content_region_avail()[0];
        self.log_slider_widget_full_width(ui, "##time_window_slider", 1.0, recording_duration_secs, slider_width);

        // Signal picker popup
        if self.show_signal_picker {
            self.render_signal_picker(ui);
        }

        // Empty state
        if self.series.is_empty() {
            ui.spacing();
            ui.text_wrapped("No signals charted. Click '+ Add Signal' to add signals from the DBC.");
            ui.spacing();
            return;
        }

        // Graph area
        let size = [ui.content_region_avail()[0], self.graph_height];
        let draw_list = ui.get_window_draw_list();
        let cursor_pos = ui.cursor_screen_pos();
        let pos_min = cursor_pos;
        let pos_max = [cursor_pos[0] + size[0], cursor_pos[1] + size[1]];

        draw_list.add_rect(pos_min, pos_max, ui.style_color(StyleColor::FrameBg))
            .filled(true).rounding(4.0).build();

        // Determine time window - show sliding window around current time
        let window_duration = Duration::seconds(self.time_window_secs as i64);

        // Get the overall data range for boundary checking
        let all_times: Vec<DateTime<Utc>> = self.series.values()
            .flat_map(|s| s.data_points.iter().map(|(_, ts)| *ts))
            .collect();

        let (data_start, data_end) = if let (Some(&first), Some(&last)) = (all_times.iter().min(), all_times.iter().max()) {
            (first, last)
        } else {
            ui.dummy(size);
            ui.text("No data");
            return;
        };

        // Debug logging
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/can-viz-chart-debug.txt") {
            let _ = writeln!(f, "Chart render: data_start={}, data_end={}, current_time={:?}",
                data_start.format("%H:%M:%S%.3f"),
                data_end.format("%H:%M:%S%.3f"),
                current_time.map(|t| t.format("%H:%M:%S%.3f").to_string())
            );
            let _ = writeln!(f, "  Total data points: {}", all_times.len());
            if let Some(&first) = all_times.first() {
                let _ = writeln!(f, "  First point: {}", first.format("%H:%M:%S%.3f"));
            }
            if let Some(&last) = all_times.last() {
                let _ = writeln!(f, "  Last point: {}", last.format("%H:%M:%S%.3f"));
            }
        }

        // Calculate display window centered on current_time (or start if no current time)
        let (time_start, time_end) = if let Some(ct) = current_time {
            let half_window = Duration::seconds((self.time_window_secs / 2.0) as i64);
            let start = (ct - half_window).max(data_start);  // Clamp to data start
            let end = start + window_duration;  // End is always window_duration from start
            (start, end)
        } else {
            // No current time, show from the beginning
            let start = data_start;
            let end = start + window_duration;
            (start, end)
        };

        // Calculate overall value range for the visible window
        let mut overall_min = f64::INFINITY;
        let mut overall_max = f64::NEG_INFINITY;
        for series in self.series.values().filter(|s| s.visible) {
            let (min, max) = series.get_value_range_in_window(time_start, time_end);
            overall_min = overall_min.min(min);
            overall_max = overall_max.max(max);
        }

        // Draw vertical grid lines (always)
        let grid_color = [0.5, 0.5, 0.5, 0.3];
        for i in 0..=10 {
            let x = pos_min[0] + (pos_max[0] - pos_min[0]) * (i as f32 / 10.0);
            draw_list.add_line([x, pos_min[1]], [x, pos_max[1]], grid_color).build();
        }

        if self.shared_y_axis {
            self.draw_grid(&draw_list, pos_min, pos_max, overall_min, overall_max);
        }

        // Draw each visible series (min-max per-pixel decimation: preserves full vertical range at every pixel column)
        for series in self.series.values() {
            if !series.visible {
                continue;
            }

            let (min_val, max_val) = if self.shared_y_axis {
                (overall_min, overall_max)
            } else {
                series.get_value_range_in_window(time_start, time_end)
            };

            let window_points: Vec<(f64, DateTime<Utc>)> = series.data_points.iter()
                .filter(|(_, ts)| *ts >= time_start && *ts <= time_end)
                .cloned()
                .collect();

            if window_points.len() < 2 {
                continue;
            }

            // Min-max decimation: envelope shows oscillation range, trend shows smooth average
            let (trend_points, envelope_lines) = self.downsample_minmax_to_screen(
                &window_points,
                time_start,
                time_end,
                min_val,
                max_val,
                pos_min,
                pos_max,
            );

            // Draw min-max envelope as filled rects (behind the trend line).
            // Each rect extends to the next occupied column, filling gaps of any size
            // caused by non-uniform CAN bus message timing at resonant zoom levels.
            if !envelope_lines.is_empty() {
                let env_color = [series.color[0], series.color[1], series.color[2], series.color[3] * 0.35];
                for i in 0..envelope_lines.len() {
                    let (x, y_min, y_max) = envelope_lines[i];
                    let top = y_min.min(y_max);
                    let bottom = y_min.max(y_max);
                    // Extend to the next envelope column to seal any gap
                    let x_right = if i + 1 < envelope_lines.len() {
                        envelope_lines[i + 1].0
                    } else {
                        x + 1.0
                    };
                    draw_list.add_rect([x - 0.5, top], [x_right + 0.5, bottom], env_color)
                        .filled(true).build();
                }
            }

            // Draw smooth trend line on top
            if trend_points.len() >= 2 {
                draw_list.add_polyline(trend_points, series.color)
                    .thickness(2.0).build();
            }
        }

        // Current time indicator - show at position within the full data range
        if let Some(ct) = current_time {
            if ct >= time_start && ct <= time_end {
                let x_pos = self.time_to_x(ct, time_start, time_end, pos_min, pos_max);
                draw_list.add_line([x_pos, pos_min[1]], [x_pos, pos_max[1]], [1.0, 1.0, 0.0, 0.8])
                    .thickness(2.0).build();
            }
        }

        // Time labels - show time position relative to data start
        let start_offset = (time_start - data_start).num_seconds() as f64;
        let end_offset = (time_end - data_start).num_seconds() as f64;
        draw_list.add_text([pos_min[0] + 5.0, pos_max[1] - 15.0], [0.6, 0.6, 0.6, 0.8],
            format!("{:.0}s", start_offset));
        draw_list.add_text([pos_max[0] - 45.0, pos_max[1] - 15.0], [0.6, 0.6, 0.6, 0.8],
            format!("{:.0}s", end_offset));

        // Draw signal-specific Y-axis labels on top (after all other drawing)
        if !self.shared_y_axis {
            self.draw_signal_y_labels(&draw_list, pos_min, pos_max, time_start, time_end);
        }

        // Reserve space for the chart
        ui.dummy(size);

        // Handle chart scrubbing - check if mouse is in the chart area
        let mouse_pos = ui.io().mouse_pos;
        let is_in_chart = mouse_pos[0] >= pos_min[0] && mouse_pos[0] <= pos_max[0] &&
                          mouse_pos[1] >= pos_min[1] && mouse_pos[1] <= pos_max[1];

        // Draw preview dashed line when hovering over chart
        if is_in_chart {
            let preview_x = mouse_pos[0];
            let preview_color = [1.0, 1.0, 1.0, 0.4];  // White with low opacity

            // Draw dashed line (simulate with short segments)
            let dash_size = 4.0;
            let gap_size = 4.0;
            let mut y = pos_min[1];
            while y < pos_max[1] {
                let segment_end = (y + dash_size).min(pos_max[1]);
                draw_list.add_line([preview_x, y], [preview_x, segment_end], preview_color)
                    .thickness(1.0).build();
                y = segment_end + gap_size;
            }

            // Handle click-to-seek - move yellow line to where the dotted line is
            if ui.is_mouse_clicked(imgui::MouseButton::Left) {
                if let Some(ct) = current_time {
                    let rel_x = (mouse_pos[0] - pos_min[0]) / (pos_max[0] - pos_min[0]);
                    if rel_x >= 0.0 && rel_x <= 1.0 {
                        // Calculate the time at the mouse position (dashed line)
                        let window_duration_ms = (time_end - time_start).num_milliseconds() as f64;
                        let offset_ms = (rel_x as f64 * window_duration_ms) as i64;
                        let mouse_time = time_start + Duration::milliseconds(offset_ms);

                        // Calculate relative offset from current time (yellow line) to mouse position
                        // Positive = mouse is to the right (forward), Negative = mouse is to the left (backward)
                        let seek_offset_secs = (mouse_time - ct).num_milliseconds() as f32 / 1000.0;

                        // Use positive value for relative seek from current time
                        self.seek_request = Some(seek_offset_secs);
                    }
                }
            }
        }

        // Legend (always shown)
        self.draw_legend(ui, time_start, time_end);
    }

    fn render_signal_picker(&mut self, ui: &Ui) {
        ui.separator();
        ui.text("Add Signal:");
        ui.same_line();

        // Filter input
        let _ = ui.input_text("##filter", &mut self.signal_filter)
            .hint("Filter signals...")
            .build();

        ui.indent();
        let filter_lower = self.signal_filter.to_lowercase();

        // Collect signals to add (can't add while iterating)
        let mut to_add: Vec<SignalInfo> = Vec::new();
        let mut to_remove: Vec<String> = Vec::new();

        for (idx, signal) in self.available_signals.iter().enumerate() {
            if !filter_lower.is_empty() {
                let name_lower = signal.name.to_lowercase();
                let msg_lower = signal.msg_name.to_lowercase();
                if !name_lower.contains(&filter_lower) && !msg_lower.contains(&filter_lower) {
                    continue;
                }
            }

            let is_charted = self.has_signal(&signal.name);
            let label = if is_charted { "[x]" } else { "[ ]" };

            let _id = ui.push_id_int(idx as i32);
            if ui.small_button(label) {
                if is_charted {
                    to_remove.push(signal.name.clone());
                } else {
                    to_add.push(signal.clone());
                }
            }
            ui.same_line();
            ui.text_colored([0.6, 0.8, 1.0, 1.0], &signal.name);
            ui.same_line();
            ui.text_colored([0.5, 0.5, 0.5, 1.0], format!("({})", signal.msg_name));
        }

        // Apply changes after iteration
        for info in to_add {
            self.add_signal(&info);
        }
        for name in to_remove {
            self.remove_signal(&name);
        }

        ui.unindent();
        ui.separator();
    }

    fn draw_grid(&self, draw_list: &imgui::DrawListMut, pos_min: [f32; 2], pos_max: [f32; 2], min_val: f64, max_val: f64) {
        let grid_color = [0.5, 0.5, 0.5, 0.3];
        for i in 0..=5 {
            let y = pos_min[1] + (pos_max[1] - pos_min[1]) * (i as f32 / 5.0);
            draw_list.add_line([pos_min[0], y], [pos_max[0], y], grid_color).build();

            let value = max_val - (max_val - min_val) * (i as f64 / 5.0);
            draw_list.add_text([pos_min[0] + 5.0, y + 2.0], [0.7, 0.7, 0.7, 0.8], format!("{:.1}", value));
        }

        for i in 0..=10 {
            let x = pos_min[0] + (pos_max[0] - pos_min[0]) * (i as f32 / 10.0);
            draw_list.add_line([x, pos_min[1]], [x, pos_max[1]], grid_color).build();
        }
    }

    /// Draw Y-axis labels for each signal when not using shared Y axis
    /// Labels are positioned horizontally at the top of the chart, each in its signal's color
    fn draw_signal_y_labels(&self, draw_list: &imgui::DrawListMut, pos_min: [f32; 2], pos_max: [f32; 2],
                              time_start: DateTime<Utc>, time_end: DateTime<Utc>) {
        // Collect series data first to avoid borrow issues
        let series_data: Vec<(String, [f32; 4], f64, f64)> = self.series.values()
            .filter(|s| s.visible)
            .map(|s| {
                let (min_val, max_val) = s.get_value_range_in_window(time_start, time_end);
                (s.name.clone(), s.color, min_val, max_val)
            })
            .collect();

        if series_data.is_empty() {
            return;
        }

        // Position labels horizontally at the top of the chart
        let start_x = pos_min[0] + 5.0;
        let y_pos = pos_min[1] + 4.0;  // Near the top
        let label_spacing = 2.0;  // Small gap between labels
        let text_height = 14.0;  // Approximate text height

        // First pass: calculate total width needed
        let mut total_width = 0.0;
        for (name, _color, _min_val, max_val) in &series_data {
            let label = format!("{:.1}", max_val);
            let text_width = label.len() as f32 * 7.0;
            total_width += text_width + label_spacing;
        }

        // Draw semi-transparent gray background behind all labels
        let bg_color = [0.1, 0.1, 0.1, 0.9];  // Dark gray with 90% opacity
        let bg_padding = 3.0;
        draw_list.add_rect(
            [start_x - bg_padding, y_pos - bg_padding],
            [start_x + total_width + bg_padding, y_pos + text_height + bg_padding],
            bg_color
        ).filled(true).rounding(3.0).build();

        // Second pass: draw the labels
        let mut x_pos = start_x;
        for (name, color, _min_val, max_val) in &series_data {
            let label = format!("{:.1}", max_val);

            // Estimate text width (approximately 7 pixels per character)
            let text_width = label.len() as f32 * 7.0;

            // Draw the label at current x position
            draw_list.add_text([x_pos, y_pos], *color, label);

            // Move x position for next label (text width + spacing)
            x_pos += text_width + label_spacing;
        }
    }

    /// Custom logarithmic slider widget
    /// Shows actual time value inside the slider with logarithmic scaling
    fn log_slider_widget(&mut self, ui: &Ui, label: &str, min: f32, max: f32) -> bool {
        let id = ui.push_id(label);
        let draw_list = ui.get_window_draw_list();
        let style = ui.clone_style();
        let cursor_pos = ui.cursor_screen_pos();

        // Slider dimensions
        let height = 14.0;
        let width = ui.content_region_avail()[0] - 45.0;
        let grab_size = 12.0;

        // Calculate logarithmic position (0-1) from current value
        let log_min = min.ln();
        let log_max = max.ln();
        let log_range = log_max - log_min;
        let log_value = self.time_window_secs.ln();
        let mut pos = ((log_value - log_min) / log_range).clamp(0.0, 1.0);

        // Background
        let bg_min = cursor_pos;
        let bg_max = [cursor_pos[0] + width, cursor_pos[1] + height];
        let bg_color = style.colors[imgui::StyleColor::FrameBg as usize];
        draw_list.add_rect(bg_min, bg_max, bg_color).filled(true).rounding(4.0).build();
        draw_list.add_rect(bg_min, bg_max, style.colors[imgui::StyleColor::Border as usize])
            .rounding(4.0).build();

        // Grab position
        let grab_x = bg_min[0] + pos * (bg_max[0] - bg_min[0]);
        let grab_min = [grab_x - grab_size / 2.0, bg_min[1] + 2.0];
        let grab_max = [grab_x + grab_size / 2.0, bg_max[1] - 2.0];

        // Check interaction state
        let mouse_pos = ui.io().mouse_pos;
        let is_hovered = mouse_pos[0] >= bg_min[0] && mouse_pos[0] <= bg_max[0] &&
                          mouse_pos[1] >= bg_min[1] && mouse_pos[1] <= bg_max[1];
        let is_clicked = is_hovered && ui.is_mouse_clicked(MouseButton::Left);
        let mouse_down = ui.is_mouse_down(MouseButton::Left);
        let mouse_released = ui.is_mouse_released(MouseButton::Left);

        // Update dragging state
        if is_clicked {
            self.slider_dragging = true;
        } else if mouse_released {
            self.slider_dragging = false;
        }

        // Active if currently being dragged
        let is_active = self.slider_dragging;

        // Grab color
        let grab_color = if is_active {
            style.colors[imgui::StyleColor::SliderGrabActive as usize]
        } else if is_hovered {
            style.colors[imgui::StyleColor::SliderGrab as usize]
        } else {
            style.colors[imgui::StyleColor::SliderGrab as usize]
        };

        draw_list.add_rect(grab_min, grab_max, grab_color).filled(true).rounding(2.0).build();

        // Handle interaction - respond while dragging (mouse held after click)
        let mut changed = false;
        if is_active {
            let rel_x = (mouse_pos[0] - bg_min[0]) / (bg_max[0] - bg_min[0]).max(0.001);
            pos = rel_x.clamp(0.0, 1.0);
            let new_log_value = log_min + pos * log_range;
            self.time_window_secs = new_log_value.exp();
            changed = true;
        }

        // Draw value text inside the slider (at the right side)
        let value_text = format!("{}s", self.time_window_secs.round() as i32);
        let text_color = style.colors[imgui::StyleColor::Text as usize];
        let text_x = bg_max[0] - value_text.len() as f32 * 7.0 - 8.0;
        let text_y = bg_min[1] + 1.0;
        draw_list.add_text([text_x, text_y], text_color, &value_text);

        // Reserve space
        ui.dummy([width, height]);

        id.pop();
        changed
    }

    /// Custom timeline slider widget with full width and time label inside
    /// Returns the new position (0-1) if changed, None otherwise
    fn timeline_slider_widget(&mut self, ui: &Ui, label: &str, current_pos: f32, total_duration_secs: f32, width: f32) -> Option<f32> {
        let id = ui.push_id(label);
        let draw_list = ui.get_window_draw_list();
        let style = ui.clone_style();
        let cursor_pos = ui.cursor_screen_pos();

        // Slider dimensions
        let height = 14.0;
        let grab_size = 12.0;

        // Background
        let bg_min = cursor_pos;
        let bg_max = [cursor_pos[0] + width, cursor_pos[1] + height];
        let bg_color = style.colors[imgui::StyleColor::FrameBg as usize];
        draw_list.add_rect(bg_min, bg_max, bg_color).filled(true).rounding(4.0).build();
        draw_list.add_rect(bg_min, bg_max, style.colors[imgui::StyleColor::Border as usize])
            .rounding(4.0).build();

        // Calculate grab position
        let grab_x = bg_min[0] + current_pos * (bg_max[0] - bg_min[0]);
        let grab_min = [grab_x - grab_size / 2.0, bg_min[1] + 2.0];
        let grab_max = [grab_x + grab_size / 2.0, bg_max[1] - 2.0];

        // Reserve space (using dummy, but we'll track mouse state manually)
        ui.dummy([width, height]);

        // Get mouse state
        let mouse_pos = ui.io().mouse_pos;
        let is_hovered = mouse_pos[0] >= bg_min[0] && mouse_pos[0] <= bg_max[0] &&
                          mouse_pos[1] >= bg_min[1] && mouse_pos[1] <= bg_max[1];
        let is_mouse_clicked = ui.is_mouse_clicked(imgui::MouseButton::Left);
        let is_mouse_down = ui.is_mouse_down(imgui::MouseButton::Left);
        let is_mouse_released = ui.is_mouse_released(imgui::MouseButton::Left);

        // Update dragging state (works even when mouse is outside)
        if is_mouse_clicked && is_hovered {
            self.timeline_dragging = true;
        }
        if is_mouse_released {
            self.timeline_dragging = false;
        }

        let is_active = self.timeline_dragging;

        // Grab color
        let grab_color = if is_active {
            style.colors[imgui::StyleColor::SliderGrabActive as usize]
        } else if is_hovered {
            style.colors[imgui::StyleColor::SliderGrab as usize]
        } else {
            style.colors[imgui::StyleColor::SliderGrab as usize]
        };

        draw_list.add_rect(grab_min, grab_max, grab_color).filled(true).rounding(2.0).build();

        // Handle interaction - work even when dragging outside the slider area
        let mut new_pos = current_pos;
        let mut changed = false;
        if is_active {
            // Calculate position based on mouse X, clamped to slider width
            let rel_x = (mouse_pos[0] - bg_min[0]) / (bg_max[0] - bg_min[0]).max(0.001);
            new_pos = rel_x.clamp(0.0, 1.0);
            if (new_pos - current_pos).abs() > 0.001 {
                changed = true;
            }
        }

        // Draw value text inside the slider (at the right side) - show current time in seconds
        let current_seconds = current_pos * total_duration_secs;
        let value_text = format!("{:.0}s", current_seconds);
        let text_color = style.colors[imgui::StyleColor::Text as usize];
        let text_x = bg_max[0] - value_text.len() as f32 * 7.0 - 8.0;
        let text_y = bg_min[1] + 1.0;
        draw_list.add_text([text_x, text_y], text_color, &value_text);

        id.pop();
        if changed { Some(new_pos) } else { None }
    }

    /// Custom logarithmic slider widget with explicit width
    /// Shows actual time value inside the slider with logarithmic scaling
    fn log_slider_widget_full_width(&mut self, ui: &Ui, label: &str, min: f32, max: f32, width: f32) -> bool {
        let id = ui.push_id(label);
        let draw_list = ui.get_window_draw_list();
        let style = ui.clone_style();
        let cursor_pos = ui.cursor_screen_pos();

        // Slider dimensions
        let height = 14.0;
        let grab_size = 12.0;

        // Calculate logarithmic position (0-1) from current value
        let log_min = min.ln();
        let log_max = max.ln();
        let log_range = log_max - log_min;
        let log_value = self.time_window_secs.ln();
        let mut pos = ((log_value - log_min) / log_range).clamp(0.0, 1.0);

        // Background
        let bg_min = cursor_pos;
        let bg_max = [cursor_pos[0] + width, cursor_pos[1] + height];
        let bg_color = style.colors[imgui::StyleColor::FrameBg as usize];
        draw_list.add_rect(bg_min, bg_max, bg_color).filled(true).rounding(4.0).build();
        draw_list.add_rect(bg_min, bg_max, style.colors[imgui::StyleColor::Border as usize])
            .rounding(4.0).build();

        // Grab position
        let grab_x = bg_min[0] + pos * (bg_max[0] - bg_min[0]);
        let grab_min = [grab_x - grab_size / 2.0, bg_min[1] + 2.0];
        let grab_max = [grab_x + grab_size / 2.0, bg_max[1] - 2.0];

        // Check interaction state
        let mouse_pos = ui.io().mouse_pos;
        let is_hovered = mouse_pos[0] >= bg_min[0] && mouse_pos[0] <= bg_max[0] &&
                          mouse_pos[1] >= bg_min[1] && mouse_pos[1] <= bg_max[1];
        let is_clicked = is_hovered && ui.is_mouse_clicked(MouseButton::Left);
        let mouse_down = ui.is_mouse_down(MouseButton::Left);
        let mouse_released = ui.is_mouse_released(MouseButton::Left);

        // Update dragging state
        if is_clicked {
            self.slider_dragging = true;
        } else if mouse_released {
            self.slider_dragging = false;
        }

        // Active if currently being dragged
        let is_active = self.slider_dragging;

        // Grab color
        let grab_color = if is_active {
            style.colors[imgui::StyleColor::SliderGrabActive as usize]
        } else if is_hovered {
            style.colors[imgui::StyleColor::SliderGrab as usize]
        } else {
            style.colors[imgui::StyleColor::SliderGrab as usize]
        };

        draw_list.add_rect(grab_min, grab_max, grab_color).filled(true).rounding(2.0).build();

        // Handle interaction - respond while dragging (mouse held after click)
        let mut changed = false;
        if is_active {
            let rel_x = (mouse_pos[0] - bg_min[0]) / (bg_max[0] - bg_min[0]).max(0.001);
            pos = rel_x.clamp(0.0, 1.0);
            let new_log_value = log_min + pos * log_range;
            self.time_window_secs = new_log_value.exp();
            changed = true;
        }

        // Draw value text inside the slider (at the right side)
        let value_text = format!("{}s", self.time_window_secs.round() as i32);
        let text_color = style.colors[imgui::StyleColor::Text as usize];
        let text_x = bg_max[0] - value_text.len() as f32 * 7.0 - 8.0;
        let text_y = bg_min[1] + 1.0;
        draw_list.add_text([text_x, text_y], text_color, &value_text);

        // Reserve space
        ui.dummy([width, height]);

        id.pop();
        changed
    }

    fn draw_legend(&mut self, ui: &Ui, time_start: DateTime<Utc>, time_end: DateTime<Utc>) {
        ui.separator();
        ui.text("Signals:");

        // Collect changes to apply after iteration
        let mut visibility_changes: Vec<(String, bool)> = Vec::new();
        let mut to_remove: Vec<String> = Vec::new();
        let series_names: Vec<String> = self.series.keys().cloned().collect();

        for (idx, name) in series_names.iter().enumerate() {
            if let Some(series) = self.series.get(name) {
                ui.same_line();
                ui.color_button("##color", series.color);
                ui.same_line();

                let mut visible = series.visible;
                let _id = ui.push_id_int(idx as i32);
                if ui.checkbox(&series.name, &mut visible) {
                    visibility_changes.push((name.clone(), visible));
                }

                ui.same_line();

                // X button to remove
                if ui.small_button("x") {
                    to_remove.push(name.clone());
                }
            }
        }

        // Apply changes after iteration
        for (name, visible) in visibility_changes {
            if let Some(s) = self.series.get_mut(&name) {
                s.visible = visible;
            }
        }
        for name in to_remove {
            self.remove_signal(&name);
        }
    }

    /// Min-max per-pixel-column decimation with two-pass output.
    /// Returns (trend_polyline, envelope_lines):
    /// - trend: smooth polyline using per-column average values (clean line like an oscilloscope)
    /// - envelope: vertical segments (x, y_min, y_max) showing the full oscillation range
    ///
    /// Rendering the envelope as individual vertical lines avoids the ImGui polyline anti-aliasing
    /// issue where steep connecting segments between pixel columns appear ghostly/dotted.
    fn downsample_minmax_to_screen(
        &self,
        points: &[(f64, DateTime<Utc>)],
        time_start: DateTime<Utc>,
        time_end: DateTime<Utc>,
        min_val: f64,
        max_val: f64,
        pos_min: [f32; 2],
        pos_max: [f32; 2],
    ) -> (Vec<[f32; 2]>, Vec<(f32, f32, f32)>) {
        let n = points.len();
        if n == 0 {
            return (vec![], vec![]);
        }

        let width = (pos_max[0] - pos_min[0]).ceil() as usize;
        if width == 0 {
            return (vec![], vec![]);
        }

        // Only skip decimation when data is truly sparse (<0.5 points per pixel).
        // Above that density, oscillations within pixel columns cause polyline zigzag artifacts.
        if n <= width / 2 {
            let trend = points.iter()
                .map(|(v, t)| {
                    let x = self.time_to_x(*t, time_start, time_end, pos_min, pos_max);
                    let y = self.value_to_y(*v, min_val, max_val, pos_min, pos_max);
                    [x, y]
                })
                .collect();
            return (trend, vec![]);
        }

        let total_duration_ms = (time_end - time_start).num_milliseconds() as f64;
        if total_duration_ms <= 0.0 {
            return (vec![], vec![]);
        }

        // Per-pixel-column bucket: tracks min/max/sum for envelope + average trend
        struct Bucket {
            min: f64,
            max: f64,
            sum: f64,
            count: usize,
        }

        // Assign every data point to its pixel column bucket
        let mut buckets: Vec<Option<Bucket>> = (0..width).map(|_| None).collect();

        for (v, t) in points.iter() {
            let elapsed_ms = (*t - time_start).num_milliseconds() as f64;
            let frac = elapsed_ms / total_duration_ms;
            let px = ((frac * width as f64).floor() as usize).min(width - 1);

            match &mut buckets[px] {
                None => {
                    buckets[px] = Some(Bucket {
                        min: *v,
                        max: *v,
                        sum: *v,
                        count: 1,
                    });
                }
                Some(b) => {
                    if *v < b.min { b.min = *v; }
                    if *v > b.max { b.max = *v; }
                    b.sum += *v;
                    b.count += 1;
                }
            }
        }

        // Build two outputs: smooth trend (averages) and envelope (min-max vertical lines)
        let mut trend: Vec<[f32; 2]> = Vec::with_capacity(width);
        let mut envelope: Vec<(f32, f32, f32)> = Vec::with_capacity(width);

        for (px, bucket) in buckets.iter().enumerate() {
            let Some(b) = bucket else { continue };

            let x = pos_min[0] + px as f32 + 0.5; // center of pixel column

            // Trend: per-column average for a smooth connected line
            let avg = b.sum / b.count as f64;
            let y_avg = self.value_to_y(avg, min_val, max_val, pos_min, pos_max);
            trend.push([x, y_avg]);

            // Envelope: min-max vertical line when there's meaningful oscillation
            if b.count > 1 {
                let y_min = self.value_to_y(b.min, min_val, max_val, pos_min, pos_max);
                let y_max = self.value_to_y(b.max, min_val, max_val, pos_min, pos_max);
                if (y_min - y_max).abs() > 1.0 {
                    envelope.push((x, y_min, y_max));
                }
            }
        }

        (trend, envelope)
    }

    fn value_to_y(&self, value: f64, min: f64, max: f64, pos_min: [f32; 2], pos_max: [f32; 2]) -> f32 {
        let range = max - min;
        if range == 0.0 {
            return (pos_min[1] + pos_max[1]) / 2.0;
        }
        let normalized = (value - min) / range;
        let clamped = normalized.clamp(0.0, 1.0);
        pos_max[1] - (clamped as f32) * (pos_max[1] - pos_min[1])
    }

    fn time_to_x(&self, time: DateTime<Utc>, time_start: DateTime<Utc>, time_end: DateTime<Utc>, pos_min: [f32; 2], pos_max: [f32; 2]) -> f32 {
        let total_duration = (time_end - time_start).num_milliseconds() as f64;
        if total_duration <= 0.0 {
            return (pos_min[0] + pos_max[0]) / 2.0;
        }
        let elapsed = (time - time_start).num_milliseconds() as f64;
        let normalized = (elapsed / total_duration).clamp(0.0, 1.0);
        pos_min[0] + (normalized as f32) * (pos_max[0] - pos_min[0])
    }
}

/// Signal browser for DBC signal selection
pub struct SignalBrowser {
    pub visible_signals: Vec<String>,
    pub selected_signal: Option<String>,
}

impl SignalBrowser {
    pub fn new() -> Self {
        Self {
            visible_signals: Vec::new(),
            selected_signal: None,
        }
    }

    pub fn add_signal(&mut self, name: &str) {
        if !self.visible_signals.contains(&name.to_string()) {
            self.visible_signals.push(name.to_string());
        }
    }

    pub fn remove_signal(&mut self, name: &str) {
        self.visible_signals.retain(|s| s != name);
    }

    pub fn toggle_signal(&mut self, name: &str) {
        if self.visible_signals.contains(&name.to_string()) {
            self.remove_signal(name);
        } else {
            self.add_signal(name);
        }
    }

    pub fn is_visible(&self, name: &str) -> bool {
        self.visible_signals.contains(&name.to_string())
    }

    pub fn render(&mut self, ui: &Ui, available_signals: &[&str]) {
        ui.text("Available Signals:");
        ui.separator();

        for signal in available_signals {
            let is_visible = self.is_visible(signal);
            let mut visible = is_visible;

            if ui.checkbox(signal, &mut visible) {
                if visible != is_visible {
                    self.toggle_signal(signal);
                }
            }

            if ui.is_item_hovered() {
                ui.tooltip(|| {
                    ui.text(format!("Signal: {}", signal));
                    ui.text("Click to toggle visibility");
                });
            }
        }
    }
}
