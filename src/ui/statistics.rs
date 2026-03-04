use imgui::{Condition, Ui, TreeNodeFlags};
use crate::core::CanMessage;
use std::collections::HashMap;

/// Message statistics calculator
pub struct MessageStatistics {
    /// Per-message-id statistics
    message_stats: HashMap<u32, MessageIdStats>,
    /// Total message count
    total_count: usize,
    /// Time range
    start_time: Option<chrono::DateTime<chrono::Utc>>,
    end_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Bus statistics
    bus_stats: HashMap<u8, usize>,
}

/// Statistics for a single message ID
#[derive(Clone, Default)]
pub struct MessageIdStats {
    pub count: usize,
    pub first_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub min_dlc: u8,
    pub max_dlc: u8,
    pub data_samples: Vec<Vec<u8>>,
    pub average_rate: f64,
}

impl MessageStatistics {
    pub fn new() -> Self {
        Self {
            message_stats: HashMap::new(),
            total_count: 0,
            start_time: None,
            end_time: None,
            bus_stats: HashMap::new(),
        }
    }

    /// Analyze a list of messages
    pub fn analyze(&mut self, messages: &[CanMessage]) {
        self.clear();

        if messages.is_empty() {
            return;
        }

        self.start_time = messages.first().map(|m| m.timestamp);
        self.end_time = messages.last().map(|m| m.timestamp);
        self.total_count = messages.len();

        for msg in messages {
            *self.bus_stats.entry(msg.bus).or_insert(0) += 1;

            let stats = self.message_stats.entry(msg.id).or_insert_with(|| {
                MessageIdStats {
                    min_dlc: 8,
                    max_dlc: 0,
                    ..Default::default()
                }
            });

            stats.count += 1;
            stats.min_dlc = stats.min_dlc.min(msg.data.len() as u8);
            stats.max_dlc = stats.max_dlc.max(msg.data.len() as u8);

            if stats.first_seen.is_none() || msg.timestamp < stats.first_seen.unwrap() {
                stats.first_seen = Some(msg.timestamp);
            }
            if stats.last_seen.is_none() || msg.timestamp > stats.last_seen.unwrap() {
                stats.last_seen = Some(msg.timestamp);
            }

            if stats.data_samples.len() < 10 {
                stats.data_samples.push(msg.data.clone());
            }
        }

        if let (Some(start), Some(end)) = (self.start_time, self.end_time) {
            let duration = (end - start).num_milliseconds() as f64 / 1000.0;
            if duration > 0.0 {
                for stats in self.message_stats.values_mut() {
                    stats.average_rate = stats.count as f64 / duration;
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.message_stats.clear();
        self.total_count = 0;
        self.start_time = None;
        self.end_time = None;
        self.bus_stats.clear();
    }

    pub fn get_message_stats(&self, id: u32) -> Option<&MessageIdStats> {
        self.message_stats.get(&id)
    }

    pub fn get_message_counts(&self) -> Vec<(u32, usize)> {
        let mut counts: Vec<_> = self.message_stats.iter()
            .map(|(&id, stats)| (id, stats.count))
            .collect();
        counts.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
        counts
    }

    pub fn total_count(&self) -> usize {
        self.total_count
    }

    pub fn unique_ids(&self) -> usize {
        self.message_stats.len()
    }

    pub fn duration_seconds(&self) -> f64 {
        if let (Some(start), Some(end)) = (self.start_time, self.end_time) {
            (end - start).num_milliseconds() as f64 / 1000.0
        } else {
            0.0
        }
    }

    pub fn average_rate(&self) -> f64 {
        let duration = self.duration_seconds();
        if duration > 0.0 {
            self.total_count as f64 / duration
        } else {
            0.0
        }
    }

    pub fn bus_distribution(&self) -> &HashMap<u8, usize> {
        &self.bus_stats
    }
}

impl Default for MessageStatistics {
    fn default() -> Self {
        Self::new()
    }
}

/// Message statistics window
pub struct MessageStatsWindow {
    stats: MessageStatistics,
    sort_by_count: bool,
    filter_text: String,
}

impl MessageStatsWindow {
    pub fn new() -> Self {
        Self {
            stats: MessageStatistics::new(),
            sort_by_count: true,
            filter_text: String::new(),
        }
    }

    pub fn update(&mut self, messages: &[CanMessage]) {
        self.stats.analyze(messages);
    }

    /// Replace stats with pre-analyzed result (for background loading)
    pub fn set_stats(&mut self, stats: MessageStatistics) {
        self.stats = stats;
    }

    pub fn clear(&mut self) {
        self.stats.clear();
    }

    pub fn render(&mut self, ui: &Ui, is_open: &mut bool) {
        ui.window("Message Statistics")
            .size([500.0, 400.0], Condition::FirstUseEver)
            .position([450.0, 30.0], Condition::FirstUseEver)
            .opened(is_open)
            .build(|| {
                self.render_content(ui);
            });
    }

    /// Render content without window wrapper - for embedding in workspace
    pub fn render_content(&mut self, ui: &Ui) {
        // Summary section
        if ui.collapsing_header("Summary", TreeNodeFlags::empty()) {
            ui.text(format!("Total Messages: {}", self.stats.total_count()));
            ui.text(format!("Unique IDs: {}", self.stats.unique_ids()));
            ui.text(format!("Duration: {:.2}s", self.stats.duration_seconds()));
            ui.text(format!("Average Rate: {:.1} msg/s", self.stats.average_rate()));

            ui.text("Bus Distribution:");
            ui.indent();
            for (&bus, &count) in self.stats.bus_distribution() {
                let pct = (count as f64 / self.stats.total_count().max(1) as f64) * 100.0;
                ui.text(format!("  Bus {}: {} ({:.1}%)", bus, count, pct));
            }
            ui.unindent();
        }

        ui.separator();

        ui.text("Message ID Statistics:");

        ui.input_text("Filter", &mut self.filter_text)
            .hint("Type to filter by ID...")
            .build();

        let mut sort_by_count = self.sort_by_count;
        ui.checkbox("Sort by Count", &mut sort_by_count);
        self.sort_by_count = sort_by_count;

        ui.separator();

        let mut counts = self.stats.get_message_counts();
        if !self.sort_by_count {
            counts.sort_by_key(|(id, _)| *id);
        }

        let filter_lower = self.filter_text.to_lowercase();
        let filtered: Vec<_> = counts.iter()
            .filter(|(id, _)| {
                if filter_lower.is_empty() {
                    true
                } else {
                    let id_str = format!("{:03x}", id);
                    id_str.contains(&filter_lower) ||
                    format!("0x{:03x}", id).contains(&filter_lower)
                }
            })
            .collect();

        // Header
        ui.text(format!("{:12} {:8} {:12} {:10}", "ID", "Count", "Rate", "DLC"));
        ui.separator();

        // Use child window for scrolling
        ui.child_window("stats_list")
            .build(|| {
                for (id, count) in &filtered {
                    if let Some(stats) = self.stats.get_message_stats(*id) {
                        let dlc_str = if stats.min_dlc == stats.max_dlc {
                            format!("{}", stats.min_dlc)
                        } else {
                            format!("{}-{}", stats.min_dlc, stats.max_dlc)
                        };

                        ui.text(format!(
                            "0x{:03X}      {:8} {:8.1}/s   {}",
                            id, count, stats.average_rate, dlc_str
                        ));
                    }
                }
            });

        ui.separator();
        if ui.button("Export to CSV") {
            println!("Export statistics to CSV");
        }
    }
}

impl Default for MessageStatsWindow {
    fn default() -> Self {
        Self::new()
    }
}

/// Data pattern analyzer
pub struct PatternAnalyzer {
    patterns: HashMap<u32, Vec<BytePattern>>,
}

#[derive(Clone)]
pub struct BytePattern {
    pub byte_index: usize,
    pub is_constant: bool,
    pub constant_value: Option<u8>,
    pub unique_values: usize,
    pub changes: usize,
}

impl PatternAnalyzer {
    pub fn new() -> Self {
        Self {
            patterns: HashMap::new(),
        }
    }

    pub fn analyze(&mut self, messages: &[CanMessage]) {
        self.patterns.clear();

        let mut by_id: HashMap<u32, Vec<&CanMessage>> = HashMap::new();
        for msg in messages {
            by_id.entry(msg.id).or_default().push(msg);
        }

        for (id, msgs) in by_id {
            if msgs.len() < 2 {
                continue;
            }

            let max_len = msgs.iter().map(|m| m.data.len()).max().unwrap_or(0);
            let mut patterns = Vec::new();

            for byte_idx in 0..max_len {
                let values: Vec<Option<u8>> = msgs.iter()
                    .map(|m| m.data.get(byte_idx).copied())
                    .collect();

                let unique: std::collections::HashSet<_> = values.iter()
                    .filter_map(|v| *v)
                    .collect();

                let changes = values.windows(2)
                    .filter(|w| w[0] != w[1])
                    .count();

                let is_constant = unique.len() == 1;
                let constant_value = if is_constant {
                    unique.iter().next().copied()
                } else {
                    None
                };

                patterns.push(BytePattern {
                    byte_index: byte_idx,
                    is_constant,
                    constant_value,
                    unique_values: unique.len(),
                    changes,
                });
            }

            self.patterns.insert(id, patterns);
        }
    }

    pub fn get_patterns(&self, id: u32) -> Option<&[BytePattern]> {
        self.patterns.get(&id).map(|v| v.as_slice())
    }

    pub fn analyzed_ids(&self) -> Vec<u32> {
        let mut ids: Vec<_> = self.patterns.keys().copied().collect();
        ids.sort();
        ids
    }

    pub fn clear(&mut self) {
        self.patterns.clear();
    }
}

impl Default for PatternAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Pattern analyzer window
pub struct PatternAnalyzerWindow {
    analyzer: PatternAnalyzer,
    selected_id: Option<u32>,
}

impl PatternAnalyzerWindow {
    pub fn new() -> Self {
        Self {
            analyzer: PatternAnalyzer::new(),
            selected_id: None,
        }
    }

    pub fn analyze(&mut self, messages: &[CanMessage]) {
        self.analyzer.analyze(messages);
    }

    /// Replace analyzer with pre-analyzed result (for background loading)
    pub fn set_analyzer(&mut self, analyzer: PatternAnalyzer) {
        self.analyzer = analyzer;
    }

    pub fn clear(&mut self) {
        self.analyzer.clear();
        self.selected_id = None;
    }

    pub fn render(&mut self, ui: &Ui, is_open: &mut bool) {
        ui.window("Pattern Analyzer")
            .size([550.0, 350.0], Condition::FirstUseEver)
            .position([450.0, 450.0], Condition::FirstUseEver)
            .opened(is_open)
            .build(|| {
                self.render_content(ui);
            });
    }

    /// Render content without window wrapper - for embedding in workspace
    pub fn render_content(&mut self, ui: &Ui) {
        ui.text("Analyze byte patterns in CAN messages");
        ui.text("Helps identify signal boundaries in unknown DBC files");
        ui.separator();

        // ID selection
        ui.text("Analyzed IDs:");
        let ids = self.analyzer.analyzed_ids();

        ui.child_window("id_list")
            .size([150.0, 300.0])
            .build(|| {
                for id in &ids {
                    let is_selected = self.selected_id == Some(*id);
                    if ui.selectable(format!("0x{:03X}", id)) {
                        self.selected_id = Some(*id);
                    }
                    if is_selected {
                        ui.set_item_default_focus();
                    }
                }
            });

        ui.same_line();

        // Show patterns for selected ID
        ui.child_window("patterns")
            .size([500.0, 300.0])
            .build(|| {
                if let Some(id) = self.selected_id {
                    if let Some(patterns) = self.analyzer.get_patterns(id) {
                        ui.text(format!("Patterns for 0x{:03X}:", id));
                        ui.separator();

                        ui.text("Byte | Type      | Unique | Changes | Value");
                        ui.separator();

                        for pattern in patterns {
                            let type_str = if pattern.is_constant {
                                "CONSTANT"
                            } else if pattern.unique_values <= 4 {
                                "FEW_VALS"
                            } else {
                                "CHANGING"
                            };

                            let value_str = if let Some(v) = pattern.constant_value {
                                format!("0x{:02X}", v)
                            } else {
                                "-".to_string()
                            };

                            let color = if pattern.is_constant {
                                [0.5, 0.5, 0.5, 1.0]
                            } else if pattern.unique_values <= 4 {
                                [0.3, 0.7, 0.3, 1.0]
                            } else {
                                [0.7, 0.7, 0.3, 1.0]
                            };

                            ui.text_colored(color, format!(
                                "  {} | {} | {:6} | {:7} | {}",
                                pattern.byte_index,
                                type_str,
                                pattern.unique_values,
                                pattern.changes,
                                value_str
                            ));
                        }

                        ui.separator();
                        ui.text_colored([0.5, 0.5, 0.5, 1.0], "CONSTANT = byte never changes");
                        ui.text_colored([0.3, 0.7, 0.3, 1.0], "FEW_VALS  = likely enum/mux");
                        ui.text_colored([0.7, 0.7, 0.3, 1.0], "CHANGING  = likely signal data");
                    }
                } else {
                    ui.text("Select a message ID to see patterns");
                }
            });
    }
}

impl Default for PatternAnalyzerWindow {
    fn default() -> Self {
        Self::new()
    }
}
