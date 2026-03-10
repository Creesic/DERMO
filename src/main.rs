#![cfg_attr(target_os = "windows", windows_subsystem = "console")]

mod core;
mod decode;
mod hardware;
mod input;
mod logging;
mod playback;
mod plugins;
mod ui;

use core::{CanMessage, DbcFile};
use decode::SignalDecoder;
use playback::PlaybackEngine;
use hardware::CanManagerCollection;
use hardware::can_manager::ManagerMessage;
use hardware::can_interface::InterfaceType;
use plugins::{PluginContext, PluginRegistry};
use ui::{MessageListWindow, FileDialogs, MultiSignalGraph, HardwareManagerWindow, LiveModeAction, LiveMessageWindow, MessageSenderWindow, MessageStatsWindow, PatternAnalyzerWindow, ShortcutManager, ExportDialog, AboutDialog, BitVisualizerWindow, SignalInfo, LogWindow};
use ui::statistics::{MessageStatistics, PatternAnalyzer};
use chrono::{DateTime, Duration, Utc};
use imgui::{Context, FontConfig, FontSource, Condition};
use imgui_winit_support::{HiDpiMode, WinitPlatform};
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoop;
use winit::window::WindowBuilder;

use glutin::prelude::*;
use glutin::display::GetGlDisplay;
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasRawWindowHandle;
use glow::HasContext;

use std::time::Instant;
use std::sync::{Arc, Mutex};
use tracing::{info, error};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::fs;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

struct AppState {
    messages: Vec<CanMessage>,
    playback: PlaybackEngine,
    message_list: MessageListWindow,
    charts: MultiSignalGraph,
    hardware_manager: HardwareManagerWindow,
    live_message_window: LiveMessageWindow,
    message_sender: MessageSenderWindow,
    initial_data_populated: bool,  // Track if we've done initial population
    /// When true, update_graphs runs even when paused (e.g. after timeline scrub)
    seek_triggered_ui_update: bool,
    // Phase 6 components
    message_stats: MessageStatsWindow,
    pattern_analyzer: PatternAnalyzerWindow,
    shortcut_manager: ShortcutManager,
    export_dialog: ExportDialog,
    about_dialog: AboutDialog,
    // Bit visualizer
    bit_visualizer: BitVisualizerWindow,
    // Log window
    log_window: LogWindow,
    dbc_file: DbcFile,
    signal_decoder: SignalDecoder,
    file_loaded: bool,
    dbc_loaded: bool,
    show_file_open_pending: bool,
    show_cabana_folder_pending: bool,
    show_dbc_open_pending: bool,
    show_save_savestate_pending: bool,
    show_load_savestate_pending: bool,
    status_message: Option<String>,
    // Incremental chart data loading
    pending_signal_loads: std::collections::HashMap<String, usize>,  // signal_name -> current message index
    // Window visibility
    show_messages: bool,
    show_charts: bool,
    show_hardware_manager: bool,
    show_live_messages: bool,
    show_message_sender: bool,
    // Phase 6 window visibility
    show_message_stats: bool,
    show_pattern_analyzer: bool,
    show_shortcuts: bool,
    // Bit visualizer visibility
    show_bit_visualizer: bool,
    // Log window
    show_log: bool,
    // Recently opened files (paths)
    recent_can_files: Vec<String>,
    recent_dbc_files: Vec<String>,
    recent_savestates: Vec<String>,
    // Savestate loading: apply when CAN load completes
    pending_savestate: Option<Savestate>,
    // Layout to apply next frame (needs imgui context)
    pending_layout_apply: Option<String>,
    // CAN hardware manager
    can_collection: CanManagerCollection,
    // Plugins
    plugin_registry: PluginRegistry,
    plugin_send_queue: Vec<(u8, CanMessage)>,
    /// Combined live + playback messages for plugins (built each frame)
    plugin_message_buffer: Vec<ManagerMessage>,
    // Async loading state
    loading: bool,
    loading_progress: f32,
    loading_total: usize,
    loading_receiver: Option<Receiver<LoadingUpdate>>,
    pending_messages: Option<Arc<Mutex<Vec<CanMessage>>>>,
    /// Receiver for background stats/analyzer results
    analysis_receiver: Option<Receiver<(MessageStatistics, PatternAnalyzer)>>,
}

/// Messages for async loading
enum LoadingUpdate {
    /// Progress(current_bytes, total_bytes) - for CSV, total_bytes is file size
    Progress(usize, usize),
    /// Chunk of messages (streaming - UI updates immediately)
    Chunk(Vec<CanMessage>),
    /// Load complete (path only - messages already sent via Chunk)
    Complete(String),
    Error(String),
}

/// Savestate: snapshot of window layout, chart signals, bit visualizer quadrants, and file paths
#[derive(Serialize, Deserialize, Default, Clone)]
struct Savestate {
    #[serde(default)]
    can_file_path: Option<String>,
    #[serde(default)]
    dbc_file_path: Option<String>,
    #[serde(default)]
    chart_signals: Vec<String>,
    /// Each quadrant: (msg_id, bus) or None for empty
    #[serde(default)]
    bit_visualizer_quadrants: Vec<(u32, u8)>,
    /// Playback position 0.0-1.0
    #[serde(default)]
    playback_position: Option<f32>,
    #[serde(default)]
    show_messages: bool,
    #[serde(default)]
    show_charts: bool,
    #[serde(default)]
    show_bit_visualizer: bool,
    #[serde(default)]
    show_hardware_manager: bool,
    #[serde(default)]
    show_live_messages: bool,
    #[serde(default)]
    show_message_sender: bool,
    #[serde(default)]
    show_message_stats: bool,
    #[serde(default)]
    show_pattern_analyzer: bool,
    #[serde(default)]
    show_log: bool,
    /// ImGui layout INI content
    #[serde(default)]
    layout_ini: String,
}

/// Persistent application settings
#[derive(Serialize, Deserialize, Default)]
struct AppSettings {
    show_messages: bool,
    show_charts: bool,
    show_hardware_manager: bool,
    show_live_messages: bool,
    show_message_sender: bool,
    show_message_stats: bool,
    show_pattern_analyzer: bool,
    show_shortcuts: bool,
    show_bit_visualizer: bool,
    show_log: bool,
    #[serde(default)]
    recent_can_files: Vec<String>,
    #[serde(default)]
    recent_dbc_files: Vec<String>,
    #[serde(default)]
    recent_savestates: Vec<String>,
}

const MAX_RECENT_FILES: usize = 10;

impl AppSettings {
    fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("can-viz").join("settings.json"))
    }

    fn load() -> Self {
        if let Some(path) = Self::config_path() {
            if path.exists() {
                if let Ok(contents) = fs::read_to_string(&path) {
                    if let Ok(settings) = serde_json::from_str(&contents) {
                        return settings;
                    }
                }
            }
        }
        // Return default with bit visualizer enabled
        Self {
            show_messages: true,
            show_charts: true,
            show_bit_visualizer: true,
            ..Default::default()
        }
    }

    fn save(&self) {
        if let Some(path) = Self::config_path() {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(self) {
                let _ = fs::write(&path, json);
            }
        }
    }
}

impl AppState {
    fn new() -> Self {
        // Load persisted settings
        let settings = AppSettings::load();

        Self {
            messages: Vec::new(),
            playback: PlaybackEngine::new(Vec::new()),
            message_list: MessageListWindow::new(),
            charts: MultiSignalGraph::new(),
            hardware_manager: HardwareManagerWindow::new(),
            live_message_window: LiveMessageWindow::new(),
            message_sender: MessageSenderWindow::new(),
            initial_data_populated: false,
            seek_triggered_ui_update: false,
            // Phase 6 components
            message_stats: MessageStatsWindow::new(),
            pattern_analyzer: PatternAnalyzerWindow::new(),
            shortcut_manager: ShortcutManager::new(),
            export_dialog: ExportDialog::new(),
            about_dialog: AboutDialog::new(),
            // Bit visualizer
            bit_visualizer: BitVisualizerWindow::new(),
            // Log window
            log_window: LogWindow::new(),
            dbc_file: DbcFile::new(),
            signal_decoder: SignalDecoder::new(),
            file_loaded: false,
            dbc_loaded: false,
            show_file_open_pending: false,
            show_cabana_folder_pending: false,
            show_dbc_open_pending: false,
            show_save_savestate_pending: false,
            show_load_savestate_pending: false,
            status_message: None,
            pending_signal_loads: std::collections::HashMap::new(),
            // Window visibility from settings
            show_messages: settings.show_messages,
            show_charts: settings.show_charts,
            show_hardware_manager: settings.show_hardware_manager,
            show_live_messages: settings.show_live_messages,
            show_message_sender: settings.show_message_sender,
            // Phase 6 window visibility
            show_message_stats: settings.show_message_stats,
            show_pattern_analyzer: settings.show_pattern_analyzer,
            show_shortcuts: settings.show_shortcuts,
            // Bit visualizer visibility
            show_bit_visualizer: settings.show_bit_visualizer,
            // Log window
            show_log: settings.show_log,
            // Recently opened files
            recent_can_files: settings.recent_can_files,
            recent_dbc_files: settings.recent_dbc_files,
            recent_savestates: settings.recent_savestates,
            pending_savestate: None,
            pending_layout_apply: None,
            // CAN hardware manager
            can_collection: CanManagerCollection::new(),
            // Plugins
            plugin_registry: PluginRegistry::new(),
            plugin_send_queue: Vec::new(),
            plugin_message_buffer: Vec::new(),
            // Async loading
            loading: false,
            loading_progress: 0.0,
            loading_total: 0,
            loading_receiver: None,
            pending_messages: None,
            analysis_receiver: None,
        }
    }

    fn save_settings(&self) {
        let settings = AppSettings {
            show_messages: self.show_messages,
            show_charts: self.show_charts,
            show_hardware_manager: self.show_hardware_manager,
            show_live_messages: self.show_live_messages,
            show_message_sender: self.show_message_sender,
            show_message_stats: self.show_message_stats,
            show_pattern_analyzer: self.show_pattern_analyzer,
            show_shortcuts: self.show_shortcuts,
            show_bit_visualizer: self.show_bit_visualizer,
            show_log: self.show_log,
            recent_can_files: self.recent_can_files.clone(),
            recent_dbc_files: self.recent_dbc_files.clone(),
            recent_savestates: self.recent_savestates.clone(),
        };
        settings.save();
    }

    fn add_recent_can_file(&mut self, path: &str) {
        let path = std::path::Path::new(path)
            .canonicalize()
            .ok()
            .and_then(|p| p.into_os_string().into_string().ok())
            .unwrap_or_else(|| path.to_string());
        self.recent_can_files.retain(|p| p != &path);
        self.recent_can_files.insert(0, path);
        if self.recent_can_files.len() > MAX_RECENT_FILES {
            self.recent_can_files.truncate(MAX_RECENT_FILES);
        }
        self.save_settings();
    }

    fn add_recent_dbc_file(&mut self, path: &str) {
        let path = std::path::Path::new(path)
            .canonicalize()
            .ok()
            .and_then(|p| p.into_os_string().into_string().ok())
            .unwrap_or_else(|| path.to_string());
        self.recent_dbc_files.retain(|p| p != &path);
        self.recent_dbc_files.insert(0, path);
        if self.recent_dbc_files.len() > MAX_RECENT_FILES {
            self.recent_dbc_files.truncate(MAX_RECENT_FILES);
        }
        self.save_settings();
    }

    fn load_file(&mut self, path: &str) {
        // Clear previous state before streaming load
        self.messages.clear();
        self.playback = PlaybackEngine::new(Vec::new());
        self.message_list.set_messages(Vec::new());
        self.file_loaded = false;
        self.pending_signal_loads.clear();
        self.charts.clear_data();
        self.charts.clear_time_range();
        self.message_stats.clear();
        self.pattern_analyzer.clear();

        // Start async streaming load
        self.loading = true;
        self.loading_progress = 0.0;
        self.loading_total = 0;
        self.status_message = Some(format!("Loading {}...", path));

        let path = path.to_string();
        let (tx, rx) = channel();
        self.loading_receiver = Some(rx);

        std::thread::spawn(move || {
            let tx_inner = std::sync::Arc::new(tx);
            let tx_chunk = tx_inner.clone();
            let tx_progress = tx_inner.clone();
            let tx_complete = tx_inner.clone();

            let chunk_cb: input::ChunkCallback = Box::new(move |msgs| {
                let _ = tx_chunk.send(LoadingUpdate::Chunk(msgs));
            });
            let progress_cb: Option<input::ProgressCallback> = Some(Box::new(move |current, total| {
                let _ = tx_progress.send(LoadingUpdate::Progress(current, total));
            }));

            match input::load_file_streaming(&path, chunk_cb, progress_cb) {
                Ok(()) => {
                    let _ = tx_complete.send(LoadingUpdate::Complete(path));
                }
                Err(e) => {
                    let _ = tx_complete.send(LoadingUpdate::Error(e.to_string()));
                }
            }
        });
    }

    fn load_cabana_folder(&mut self, folder_path: &str) {
        self.messages.clear();
        self.playback = PlaybackEngine::new(Vec::new());
        self.message_list.set_messages(Vec::new());
        self.file_loaded = false;
        self.pending_signal_loads.clear();
        self.charts.clear_data();
        self.charts.clear_time_range();
        self.message_stats.clear();
        self.pattern_analyzer.clear();

        self.loading = true;
        self.loading_progress = 0.0;
        self.loading_total = 0;
        self.status_message = Some(format!("Loading Cabana session {}...", folder_path));

        let folder_path = folder_path.to_string();
        let (tx, rx) = channel();
        self.loading_receiver = Some(rx);

        std::thread::spawn(move || {
            let tx_chunk = tx.clone();
            let tx_complete = tx.clone();

            match input::load_cabana_session(&folder_path) {
                Ok(msgs) => {
                    let _ = tx_chunk.send(LoadingUpdate::Chunk(msgs));
                    let _ = tx_complete.send(LoadingUpdate::Complete(folder_path));
                }
                Err(e) => {
                    let _ = tx_complete.send(LoadingUpdate::Error(e.to_string()));
                }
            }
        });
    }

    /// Process loading updates from background thread
    fn process_loading(&mut self) {
        // Take the receiver to avoid borrow issues
        let receiver = match self.loading_receiver.take() {
            Some(r) => r,
            None => return,
        };

        // Non-blocking check for updates
        let mut done = false;
        let mut should_restore = true;

        while let Ok(update) = receiver.try_recv() {
            match update {
                LoadingUpdate::Progress(current, total) => {
                    self.loading_progress = if total > 0 {
                        (current as f32 / total as f32) * 100.0
                    } else {
                        0.0
                    };
                    self.loading_total = total;
                    let mb_current = current as f32 / 1_000_000.0;
                    let mb_total = total as f32 / 1_000_000.0;
                    self.status_message = Some(format!(
                        "Loading... {:.0}% ({:.1}/{:.1} MB)",
                        self.loading_progress, mb_current, mb_total
                    ));
                }
                LoadingUpdate::Chunk(msgs) => {
                    self.apply_chunk(&msgs);
                }
                LoadingUpdate::Complete(path) => {
                    self.finish_streaming_load(&path);
                    if let Some(savestate) = self.pending_savestate.take() {
                        self.apply_savestate(&savestate);
                    }
                    self.loading = false;
                    done = true;
                    should_restore = false;
                }
                LoadingUpdate::Error(e) => {
                    self.status_message = Some(format!("Failed to load file: {}", e));
                    self.loading = false;
                    done = true;
                    should_restore = false;
                }
            }
            if done {
                break;
            }
        }

        // Restore receiver if not done
        if should_restore {
            self.loading_receiver = Some(receiver);
        }
    }

    /// Apply a chunk of messages from streaming load - UI updates immediately
    fn apply_chunk(&mut self, msgs: &[CanMessage]) {
        let is_first = self.messages.is_empty();

        self.messages.extend_from_slice(msgs);
        self.playback.append_messages(msgs);
        self.message_list.append_messages(msgs);

        if is_first {
            self.file_loaded = true;
            self.initial_data_populated = false;
            if let (Some(first), Some(last)) = (msgs.first(), msgs.last()) {
                self.charts.set_data_time_range(first.timestamp, last.timestamp);
            }
            self.charts.clear_data();
            if self.dbc_loaded {
                for key in self.charts.charted_signals() {
                    self.pending_signal_loads.insert(key.to_string(), 0);
                }
            }
        } else if let (Some(first), Some(last)) = (self.messages.first(), self.messages.last()) {
            self.charts.set_data_time_range(first.timestamp, last.timestamp);
        }
    }

    /// Finish streaming load (all chunks received)
    fn finish_streaming_load(&mut self, path: &str) {
        self.add_recent_can_file(path);
        let msg_count = self.messages.len();

        let messages = self.messages.clone();
        let (tx, rx) = channel();
        self.analysis_receiver = Some(rx);
        std::thread::spawn(move || {
            let mut stats = MessageStatistics::new();
            stats.analyze(&messages);
            let mut analyzer = PatternAnalyzer::new();
            analyzer.analyze(&messages);
            let _ = tx.send((stats, analyzer));
        });

        self.status_message = Some(format!("Loaded {} messages", msg_count));
        info!("Loaded {} messages", msg_count);
    }

    /// Process background analysis results (stats + pattern analyzer)
    fn process_analysis_results(&mut self) {
        let receiver = match self.analysis_receiver.take() {
            Some(r) => r,
            None => return,
        };
        if let Ok((stats, analyzer)) = receiver.try_recv() {
            self.message_stats.set_stats(stats);
            self.pattern_analyzer.set_analyzer(analyzer);
        } else {
            self.analysis_receiver = Some(receiver);
        }
    }

    /// Finish loading after background thread completes
    fn finish_loading(&mut self, messages: Vec<CanMessage>, path: &str) {
        self.add_recent_can_file(path);
        let msg_count = messages.len();
        self.messages = messages.clone();
        self.playback = PlaybackEngine::new(messages.clone());
        self.message_list.set_messages(messages.clone());
        self.file_loaded = true;
        self.initial_data_populated = false;  // Reset for initial population

        // Set data time range for charts timeline
        if let (Some(first), Some(last)) = (messages.first(), messages.last()) {
            self.charts.set_data_time_range(first.timestamp, last.timestamp);
        }

        // Clear chart data but keep selected signals
        self.charts.clear_data();

        // Defer chart population to incremental loading (like "Add to chart") - prevents UI freeze
        if self.dbc_loaded {
            for key in self.charts.charted_signals() {
                self.pending_signal_loads.insert(key.to_string(), 0);
            }
        }

        // Defer stats/analyzer to background thread - prevents main thread freeze
        let messages_for_analysis = messages.clone();
        let (tx, rx) = channel();
        self.analysis_receiver = Some(rx);
        std::thread::spawn(move || {
            let mut stats = MessageStatistics::new();
            stats.analyze(&messages_for_analysis);
            let mut analyzer = PatternAnalyzer::new();
            analyzer.analyze(&messages_for_analysis);
            let _ = tx.send((stats, analyzer));
        });

        self.status_message = Some(format!("Loaded {} messages", msg_count));
        info!("Loaded {} messages", msg_count);
    }

    /// Unload the currently loaded file
    fn unload_file(&mut self) {
        self.messages.clear();
        self.playback = PlaybackEngine::new(Vec::new());
        self.message_list.set_messages(Vec::new());
        self.file_loaded = false;
        self.initial_data_populated = false;

        // Clear chart data and timeline
        self.charts.clear_data();
        self.charts.clear_time_range();

        // Clear message stats and pattern analyzer
        self.message_stats.clear();
        self.pattern_analyzer.clear();

        self.status_message = Some("File unloaded".to_string());
    }

    /// Pre-populate chart with all decoded signal data from loaded messages
    fn populate_chart_data(&mut self) {
        let charted: Vec<String> = self.charts.charted_signals().iter().map(|s| s.to_string()).collect();
        if charted.is_empty() {
            return;
        }

        for msg in &self.messages {
            let signals = self.signal_decoder.decode_message(&msg);
            for signal in &signals {
                let key = format!("{}@bus{}", signal.name, msg.bus);
                if charted.contains(&key) {
                    self.charts.add_point(&key, signal.physical_value, msg.timestamp);
                }
            }
        }
    }

    /// Populate chart data for a specific signal (bus-aware key: "name@busN")
    fn populate_chart_data_for_signal(&mut self, signal_key: &str) {
        // Parse the bus-aware signal key
        let (signal_name, bus) = if let Some(pos) = signal_key.find("@bus") {
            (&signal_key[..pos], signal_key[pos + 4..].parse::<u8>().unwrap_or(0))
        } else {
            (signal_key, 0)
        };

        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/can-viz-chart-debug.txt")
            .ok();
        if let Some(ref mut f) = f {
            let _ = writeln!(f, "populate_chart_data_for_signal: key={}, name={}, bus={}", signal_key, signal_name, bus);
            let _ = writeln!(f, "  file_loaded: {}, dbc_loaded: {}", self.file_loaded, self.dbc_loaded);
        }

        if !self.file_loaded || !self.dbc_loaded {
            if let Some(ref mut f) = f { let _ = writeln!(f, "  returning early - files not loaded"); }
            return;
        }

        // Start incremental loading - begin at message index 0
        self.pending_signal_loads.insert(signal_key.to_string(), 0);
        if let Some(ref mut f) = f { let _ = writeln!(f, "  started incremental loading for {}", signal_key); }
    }

    // Process a batch of pending signal data loading (call this each frame)
    fn process_pending_signal_loads(&mut self) {
        const BATCH_SIZE: usize = 10000; // Process up to 10k messages per frame per signal

        let mut completed = Vec::new();

        for (signal_key, start_idx) in self.pending_signal_loads.iter_mut() {
            // Parse the bus-aware signal key
            let (signal_name, bus) = if let Some(pos) = signal_key.find("@bus") {
                (&signal_key[..pos], signal_key[pos + 4..].parse::<u8>().unwrap_or(0))
            } else {
                (signal_key.as_str(), 0)
            };

            let end_idx = (*start_idx + BATCH_SIZE).min(self.messages.len());

            for msg_idx in *start_idx..end_idx {
                if let Some(msg) = self.messages.get(msg_idx) {
                    // Only add data from messages on the correct bus
                    if msg.bus == bus {
                        let signals = self.signal_decoder.decode_message(&msg);
                        for signal in &signals {
                            if signal.name == signal_name {
                                self.charts.add_point(signal_key, signal.physical_value, msg.timestamp);
                            }
                        }
                    }
                }
            }

            *start_idx = end_idx;

            if end_idx >= self.messages.len() {
                completed.push(signal_key.clone());
            }
        }

        // Remove completed loads
        for key in completed {
            self.pending_signal_loads.remove(&key);
        }
    }

    fn load_dbc(&mut self, path: &str) {
        match DbcFile::load(path) {
            Ok(dbc) => {
                self.add_recent_dbc_file(path);
                self.signal_decoder.set_dbc(dbc.clone());
                self.dbc_file = dbc.clone();
                self.message_list.set_dbc(dbc.clone());
                self.dbc_loaded = true;

                // Populate available signals for charts
                let mut signals = Vec::new();
                for msg in &dbc.messages {
                    for sig in &msg.signals {
                        signals.push(SignalInfo {
                            name: sig.name.clone(),
                            msg_id: msg.id,
                            bus: 0,  // TODO: support per-bus DBC definitions in the future
                            msg_name: msg.name.clone(),
                            unit: sig.unit.clone().unwrap_or_default(),
                        });
                    }
                }
                self.charts.set_available_signals(signals);

                // Pre-populate chart with all data if log file is already loaded
                if self.file_loaded {
                    self.populate_chart_data();
                }

                self.status_message = Some(format!("Loaded DBC: {} messages defined", self.dbc_file.messages.len()));
                info!("Loaded DBC with {} messages", self.dbc_file.messages.len());
            }
            Err(e) => {
                self.status_message = Some(format!("Failed to load DBC: {}", e));
                error!("Failed to load DBC: {}", e);
            }
        }
    }

    fn process_file_dialogs(&mut self) {
        // Handle file open dialog
        if self.show_file_open_pending {
            if let Some(path) = FileDialogs::open_can_file() {
                self.load_file(path.to_str().unwrap_or(""));
            }
            self.show_file_open_pending = false;
        }

        if self.show_cabana_folder_pending {
            if let Some(path) = FileDialogs::open_cabana_session_folder() {
                self.load_cabana_folder(path.to_str().unwrap_or(""));
            }
            self.show_cabana_folder_pending = false;
        }

        // Handle DBC open dialog
        if self.show_dbc_open_pending {
            if let Some(path) = FileDialogs::open_dbc_file() {
                self.load_dbc(path.to_str().unwrap_or(""));
            }
            self.show_dbc_open_pending = false;
        }

        // Handle load savestate dialog
        if self.show_load_savestate_pending {
            if let Some(path) = FileDialogs::open_savestate_file() {
                self.load_savestate(path.to_str().unwrap_or(""));
            }
            self.show_load_savestate_pending = false;
        }
    }

    fn process_savestate_save(&mut self, imgui: &mut imgui::Context) {
        if !self.show_save_savestate_pending {
            return;
        }
        self.show_save_savestate_pending = false;
        if let Some(path) = FileDialogs::save_savestate_file() {
            let mut layout_ini = String::new();
            imgui.save_ini_settings(&mut layout_ini);

            let can_path = if self.file_loaded {
                self.recent_can_files.first().cloned()
            } else {
                None
            };
            let dbc_path = if self.dbc_loaded {
                self.recent_dbc_files.first().cloned()
            } else {
                None
            };

            let playback_pos = self.playback.current_time().and_then(|ct| {
                let messages = &self.messages;
                if messages.is_empty() {
                    return None;
                }
                let (first, last) = (messages.first()?, messages.last()?);
                let total = (last.timestamp - first.timestamp).num_milliseconds() as f64;
                if total <= 0.0 {
                    return None;
                }
                let elapsed = (ct - first.timestamp).num_milliseconds() as f64;
                Some((elapsed / total) as f32)
            });

            let savestate = Savestate {
                can_file_path: can_path,
                dbc_file_path: dbc_path,
                chart_signals: self.charts.get_charted_signals(),
                bit_visualizer_quadrants: self.bit_visualizer.get_quadrant_selections(),
                playback_position: playback_pos,
                show_messages: self.show_messages,
                show_charts: self.show_charts,
                show_bit_visualizer: self.show_bit_visualizer,
                show_hardware_manager: self.show_hardware_manager,
                show_live_messages: self.show_live_messages,
                show_message_sender: self.show_message_sender,
                show_message_stats: self.show_message_stats,
                show_pattern_analyzer: self.show_pattern_analyzer,
                show_log: self.show_log,
                layout_ini,
            };

            if let Ok(json) = serde_json::to_string_pretty(&savestate) {
                if fs::write(&path, json).is_ok() {
                    self.add_recent_savestate(path.to_str().unwrap_or(""));
                    self.status_message = Some("Savestate saved".to_string());
                } else {
                    self.status_message = Some("Failed to write savestate".to_string());
                }
            } else {
                self.status_message = Some("Failed to serialize savestate".to_string());
            }
        }
    }

    fn add_recent_savestate(&mut self, path: &str) {
        let path = std::path::Path::new(path)
            .canonicalize()
            .ok()
            .and_then(|p| p.into_os_string().into_string().ok())
            .unwrap_or_else(|| path.to_string());
        self.recent_savestates.retain(|p| p != &path);
        self.recent_savestates.insert(0, path);
        if self.recent_savestates.len() > MAX_RECENT_FILES {
            self.recent_savestates.truncate(MAX_RECENT_FILES);
        }
        self.save_settings();
    }

    fn load_savestate(&mut self, path: &str) {
        let contents = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => {
                self.status_message = Some("Failed to read savestate file".to_string());
                return;
            }
        };
        let savestate: Savestate = match serde_json::from_str(&contents) {
            Ok(s) => s,
            Err(_) => {
                self.status_message = Some("Invalid savestate format".to_string());
                return;
            }
        };

        self.add_recent_savestate(path);

        // Load DBC first (needed for chart signals)
        if let Some(ref dbc_path) = savestate.dbc_file_path {
            if std::path::Path::new(dbc_path).exists() {
                self.load_dbc(dbc_path);
            }
        }

        // Load CAN file if present (async)
        if let Some(ref can_path) = savestate.can_file_path {
            if std::path::Path::new(can_path).exists() {
                let path = can_path.clone();
                self.pending_savestate = Some(savestate);
                self.load_file(&path);
                return;
            }
        }

        // No CAN file or path missing - apply savestate immediately
        self.apply_savestate(&savestate);
    }

    fn apply_savestate(&mut self, savestate: &Savestate) {
        // Window visibility
        self.show_messages = savestate.show_messages;
        self.show_charts = savestate.show_charts;
        self.show_bit_visualizer = savestate.show_bit_visualizer;
        self.show_hardware_manager = savestate.show_hardware_manager;
        self.show_live_messages = savestate.show_live_messages;
        self.show_message_sender = savestate.show_message_sender;
        self.show_message_stats = savestate.show_message_stats;
        self.show_pattern_analyzer = savestate.show_pattern_analyzer;
        self.show_log = savestate.show_log;

        // Chart signals (requires DBC to be loaded)
        if self.dbc_loaded {
            self.charts.restore_signals(&savestate.chart_signals);
            if self.file_loaded {
                self.populate_chart_data();
            }
        }

        // Bit visualizer quadrants
        self.bit_visualizer.set_quadrant_selections(&savestate.bit_visualizer_quadrants);

        // Playback position
        if let (Some(pos), Some(first), Some(last)) = (
            savestate.playback_position,
            self.messages.first(),
            self.messages.last(),
        ) {
            let total_ms = (last.timestamp - first.timestamp).num_milliseconds() as f64;
            let offset_ms = (pos as f64 * total_ms) as i64;
            let target = first.timestamp + chrono::Duration::milliseconds(offset_ms);
            self.playback.seek_to_time(Some(target));
        }

        // Layout - apply next frame (needs imgui)
        if !savestate.layout_ini.is_empty() {
            self.pending_layout_apply = Some(savestate.layout_ini.clone());
        }

        self.status_message = Some("Savestate loaded".to_string());
    }

    fn update_graphs(&mut self) {
        if !self.file_loaded {
            return;
        }

        // Update when playing, on initial population, or after a seek (e.g. timeline scrub while paused)
        let is_initial_pop = !self.initial_data_populated && self.playback.current_time().is_some();
        let seek_triggered = self.seek_triggered_ui_update;
        if !self.playback.is_playing() && !is_initial_pop && !seek_triggered {
            return;
        }
        if seek_triggered {
            self.seek_triggered_ui_update = false;
        }

        if let Some(_current_time) = self.playback.current_time() {
            let window_msgs = self.playback.get_window(
                chrono::Duration::milliseconds(100),
                chrono::Duration::seconds(0),
            );

            // Update message list (live mode)
            for msg in window_msgs {
                self.message_list.update_message(msg);
            }
        }

        // Mark initial population as done
        if !self.playback.is_playing() {
            self.initial_data_populated = true;
        }
    }
}

fn main() {
    // Initialize logging: console (stderr), file, and in-app buffer
    logging::init();

    // Install panic hook to capture panics to log file
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        tracing::error!("PANIC: {}", panic_info);
        default_hook(panic_info);
    }));

    info!("DERMO v{} starting", env!("CARGO_PKG_VERSION"));

    // Create tokio runtime for async operations
    let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");

    // Create event loop
    let event_loop = EventLoop::new().expect("Failed to create EventLoop");

    // Build the window and GL display using glutin-winit
    let (window, gl_config) = DisplayBuilder::new()
        .with_window_builder(Some(
            WindowBuilder::new()
                .with_title("DERMO - Data Extraction and Real-Time Message Observer")
                .with_inner_size(winit::dpi::LogicalSize::new(1400.0, 900.0))
        ))
        .build(&event_loop, glutin::config::ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(Some(true)), |mut iter| {
            iter.next().unwrap()
        })
        .expect("Failed to create window and display");

    let window = window.expect("Failed to create window");
    let gl_display = gl_config.display();

    // Create the context using the proper API
    let context = unsafe {
        gl_display.create_context(
            &gl_config,
            &glutin::context::ContextAttributesBuilder::new()
                .build(Some(window.raw_window_handle())),
        )
    }.expect("Failed to create GL context");

    // Create surface and make context current
    let attrs = window.build_surface_attributes(
        glutin::surface::SurfaceAttributesBuilder::<glutin::surface::WindowSurface>::new()
    );

    let surface = unsafe {
        gl_display.create_window_surface(&gl_config, &attrs)
    }.expect("Failed to create surface");

    let context = context.make_current(&surface).expect("Failed to make context current");

    // Create glow context for renderer
    let gl = unsafe {
        glow::Context::from_loader_function(|ptr| {
            gl_display.get_proc_address(&std::ffi::CString::new(ptr).unwrap()) as *const _
        })
    };

    // Set up imgui
    let mut imgui = Context::create();

    // Disable ImGui debug log window
    imgui.set_log_filename(None::<std::path::PathBuf>);

    // Re-enable ini file for saving window positions
    let ini_path = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("can-viz")
        .join("layout.ini");

    if let Some(parent) = ini_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // If no user layout exists, copy the default layout
    if !ini_path.exists() {
        // Try to find default_layout.ini next to the executable or in current dir
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));
        
        let default_layout_paths: Vec<std::path::PathBuf> = vec![
            exe_dir.map(|p| p.join("default_layout.ini")).unwrap_or_default(),
            std::path::PathBuf::from("default_layout.ini"),
        ];

        for default_path in default_layout_paths {
            if default_path.exists() {
                if let Ok(contents) = std::fs::read_to_string(&default_path) {
                    let _ = std::fs::write(&ini_path, contents);
                    break;
                }
            }
        }
    }

    imgui.set_ini_filename(Some(ini_path));

    // Disable debug log via FFI
    unsafe {
        // Get current context and set ShowDebugLog to false
        let ctx = imgui::sys::igGetCurrentContext();
        if !ctx.is_null() {
            // The DebugMetricsConfig.ShowDebugLog field controls the debug log window
            (*ctx).DebugMetricsConfig.ShowDebugLog = false;
        }
    }

    // Enable docking
    imgui.io_mut().config_flags |= imgui::ConfigFlags::DOCKING_ENABLE;

    // Configure fonts
    let mut hidpi_factor = window.scale_factor();
    let font_size = (14.0 * hidpi_factor) as f32;
    imgui.fonts().add_font(&[FontSource::DefaultFontData {
        config: Some(FontConfig {
            size_pixels: font_size,
            ..FontConfig::default()
        }),
    }]);
    imgui.io_mut().font_global_scale = (1.0 / hidpi_factor) as f32;

    // Set up platform and renderer
    let mut platform = WinitPlatform::init(&mut imgui);
    platform.attach_window(imgui.io_mut(), &window, HiDpiMode::Default);

    let mut renderer = imgui_glow_renderer::AutoRenderer::initialize(gl, &mut imgui)
        .expect("Failed to initialize renderer");

    // Create a second glow context for clearing (both reference the same GL context)
    let gl_clear = unsafe {
        glow::Context::from_loader_function(|ptr| {
            gl_display.get_proc_address(&std::ffi::CString::new(ptr).unwrap()) as *const _
        })
    };

    // Create app state
    let mut state = AppState::new();
    let mut last_frame_time = Instant::now();
    let mut last_settings_save = Instant::now();

    // Main loop
    event_loop.run(move |event, window_target| {
        match event {
            Event::NewEvents(_) => {
                let now = Instant::now();
                imgui.io_mut().update_delta_time(now - last_frame_time);
                last_frame_time = now;
            }
            Event::AboutToWait => {
                // Process file dialogs
                state.process_file_dialogs();
                state.process_savestate_save(&mut imgui);

                // Apply pending layout from savestate load
                if let Some(layout) = state.pending_layout_apply.take() {
                    imgui.load_ini_settings(&layout);
                }

                // Process async loading
                state.process_loading();

                // Process background analysis results
                state.process_analysis_results();

                // Update playback
                state.playback.update(std::time::Duration::from_millis(16));

                // Update graphs with decoded signals
                state.update_graphs();

                // Save settings periodically (every 30 seconds)
                if last_settings_save.elapsed().as_secs() >= 30 {
                    state.save_settings();
                    last_settings_save = Instant::now();
                }

                platform.prepare_frame(imgui.io_mut(), &window)
                    .expect("Failed to prepare frame");
                window.request_redraw();
            }
            Event::WindowEvent { event: WindowEvent::RedrawRequested, .. } => {
                let ui = imgui.new_frame();

                // Hide the Debug window by moving it off-screen and collapsing it
                unsafe {
                    use std::ffi::CString;
                    let debug_window_name = CString::new("Debug##Default").unwrap();

                    // Move the Debug window way off-screen
                    let off_screen_pos = imgui::sys::ImVec2 { x: -10000.0, y: -10000.0 };
                    imgui::sys::igSetWindowPos_Str(
                        debug_window_name.as_ptr(),
                        off_screen_pos,
                        imgui::sys::ImGuiCond_Always as imgui::sys::ImGuiCond
                    );

                    // Collapse it
                    imgui::sys::igSetWindowCollapsed_Str(
                        debug_window_name.as_ptr(),
                        true,
                        imgui::sys::ImGuiCond_Always as imgui::sys::ImGuiCond
                    );

                    // Also clear the debug log buffer
                    let ctx = imgui::sys::igGetCurrentContext();
                    if !ctx.is_null() {
                        (*ctx).DebugMetricsConfig.ShowDebugLog = false;
                        imgui::sys::ImGuiTextBuffer_clear(&mut (*ctx).DebugLogBuf);
                    }
                }

                // Menu bar
                ui.main_menu_bar(|| {
                    ui.menu("File", || {
                        if ui.menu_item("Open CAN Log...") {
                            state.show_file_open_pending = true;
                        }
                        if ui.menu_item("Open Cabana Session...") {
                            state.show_cabana_folder_pending = true;
                        }
                        if ui.menu_item("Load DBC...") {
                            state.show_dbc_open_pending = true;
                        }
                        if ui.menu_item("Save DBC...") {
                            if let Some(path) = FileDialogs::save_dbc_file() {
                                if let Some(path_str) = path.to_str() {
                                    match state.dbc_file.save(&path) {
                                        Ok(()) => {
                                            state.add_recent_dbc_file(path_str);
                                            state.status_message = Some(format!("Saved DBC to {}", path_str));
                                        }
                                        Err(e) => {
                                            state.status_message = Some(format!("Failed to save DBC: {}", e));
                                        }
                                    }
                                }
                            }
                        }
                        if ui.menu_item("Export to CSV...") {
                            state.export_dialog.show();
                        }
                        ui.separator();
                        if let Some(_menu) = ui.begin_menu("Recently opened") {
                            let has_recent = !state.recent_can_files.is_empty() || !state.recent_dbc_files.is_empty();
                            if !has_recent {
                                ui.text_disabled("No recent files");
                            } else {
                                for path in state.recent_can_files.clone() {
                                    let display = std::path::Path::new(&path)
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or(&path)
                                        .to_string();
                                    let label = format!("{}##can_{}", display, path);
                                    if std::path::Path::new(&path).exists() {
                                        if ui.menu_item(&label) {
                                            if std::path::Path::new(&path).is_dir() {
                                                state.load_cabana_folder(&path);
                                            } else {
                                                state.load_file(&path);
                                            }
                                        }
                                    } else {
                                        ui.text_disabled(&format!("{} (missing)", display));
                                    }
                                }
                                if !state.recent_can_files.is_empty() && !state.recent_dbc_files.is_empty() {
                                    ui.separator();
                                }
                                for path in state.recent_dbc_files.clone() {
                                    let display = std::path::Path::new(&path)
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or(&path)
                                        .to_string();
                                    let label = format!("{}##dbc_{}", display, path);
                                    if std::path::Path::new(&path).exists() {
                                        if ui.menu_item(&label) {
                                            state.load_dbc(&path);
                                        }
                                    } else {
                                        ui.text_disabled(&format!("{} (missing)", display));
                                    }
                                }
                            }
                        }
                        ui.separator();
                        ui.separator();
                        if ui.menu_item("Save Savestate...") {
                            state.show_save_savestate_pending = true;
                        }
                        if ui.menu_item("Load Savestate...") {
                            state.show_load_savestate_pending = true;
                        }
                        if let Some(_menu) = ui.begin_menu("Recent Savestates") {
                            if state.recent_savestates.is_empty() {
                                ui.text_disabled("No recent savestates");
                            } else {
                                for path in state.recent_savestates.clone() {
                                    let display = std::path::Path::new(&path)
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or(&path)
                                        .to_string();
                                    let label = format!("{}##savestate_{}", display, path);
                                    if std::path::Path::new(&path).exists() {
                                        if ui.menu_item(&label) {
                                            state.load_savestate(&path);
                                        }
                                    } else {
                                        ui.text_disabled(&format!("{} (missing)", display));
                                    }
                                }
                            }
                        }
                        ui.separator();
                        if state.file_loaded {
                            if ui.menu_item("Unload") {
                                state.unload_file();
                            }
                            ui.separator();
                        }
                        if ui.menu_item("Exit") {
                            window_target.exit();
                        }
                    });

                    ui.menu("Playback", || {
                        if ui.menu_item("Play") {
                            state.playback.play();
                        }
                        if ui.menu_item("Pause") {
                            state.playback.pause();
                        }
                        if ui.menu_item("Stop") {
                            state.playback.stop();
                        }
                        ui.separator();
                        ui.text(format!("Speed: {:.1}x", state.playback.speed()));
                    });

                    ui.menu("View", || {
                        let _tok = if state.show_messages { Some(ui.push_style_color(imgui::StyleColor::Text, [0.0, 1.0, 0.0, 1.0])) } else { None };
                        if ui.menu_item("Messages") {
                            state.show_messages = !state.show_messages;
                        }
                        drop(_tok);

                        let _tok = if state.show_charts { Some(ui.push_style_color(imgui::StyleColor::Text, [0.0, 1.0, 0.0, 1.0])) } else { None };
                        if ui.menu_item("Charts") {
                            state.show_charts = !state.show_charts;
                        }
                        drop(_tok);

                        ui.separator();

                        // Hardware/Live Mode windows
                        let _tok = if state.show_hardware_manager { Some(ui.push_style_color(imgui::StyleColor::Text, [0.0, 1.0, 0.0, 1.0])) } else { None };
                        if ui.menu_item("Hardware Manager") {
                            state.show_hardware_manager = !state.show_hardware_manager;
                        }
                        drop(_tok);

                        let _tok = if state.show_live_messages { Some(ui.push_style_color(imgui::StyleColor::Text, [0.0, 1.0, 0.0, 1.0])) } else { None };
                        if ui.menu_item("Live Messages") {
                            state.show_live_messages = !state.show_live_messages;
                        }
                        drop(_tok);

                        let _tok = if state.show_message_sender { Some(ui.push_style_color(imgui::StyleColor::Text, [0.0, 1.0, 0.0, 1.0])) } else { None };
                        if ui.menu_item("Message Sender") {
                            state.show_message_sender = !state.show_message_sender;
                        }
                        drop(_tok);

                        ui.separator();

                        // Analysis windows
                        let _tok = if state.show_message_stats { Some(ui.push_style_color(imgui::StyleColor::Text, [0.0, 1.0, 0.0, 1.0])) } else { None };
                        if ui.menu_item("Message Statistics") {
                            state.show_message_stats = !state.show_message_stats;
                        }
                        drop(_tok);

                        let _tok = if state.show_pattern_analyzer { Some(ui.push_style_color(imgui::StyleColor::Text, [0.0, 1.0, 0.0, 1.0])) } else { None };
                        if ui.menu_item("Pattern Analyzer") {
                            state.show_pattern_analyzer = !state.show_pattern_analyzer;
                        }
                        drop(_tok);

                        ui.separator();

                        // Bit Visualizer
                        let _tok = if state.show_bit_visualizer { Some(ui.push_style_color(imgui::StyleColor::Text, [0.0, 1.0, 0.0, 1.0])) } else { None };
                        if ui.menu_item("Bit Visualizer") {
                            state.show_bit_visualizer = !state.show_bit_visualizer;
                        }
                        drop(_tok);

                        ui.separator();

                        // Log (console output)
                        let _tok = if state.show_log { Some(ui.push_style_color(imgui::StyleColor::Text, [0.0, 1.0, 0.0, 1.0])) } else { None };
                        if ui.menu_item("Log") {
                            state.show_log = !state.show_log;
                        }
                        drop(_tok);
                    });

                    ui.menu("Plugins", || {
                        let plugin_items: Vec<(String, String)> = state.plugin_registry.plugins()
                            .map(|(id, name, _)| (id.to_string(), name.to_string()))
                            .collect();
                        for (id, name) in plugin_items {
                            let visible = state.plugin_registry.is_visible(&id);
                            let _tok = if visible { Some(ui.push_style_color(imgui::StyleColor::Text, [0.0, 1.0, 0.0, 1.0])) } else { None };
                            if ui.menu_item(&name) {
                                state.plugin_registry.toggle_visible(&id);
                            }
                            drop(_tok);
                        }
                    });

                    ui.menu("Help", || {
                        if ui.menu_item("Keyboard Shortcuts") {
                            state.show_shortcuts = true;
                        }
                        ui.separator();
                        if ui.menu_item("About DERMO") {
                            state.about_dialog.show();
                        }
                    });

                    // Version display on the right
                    ui.same_line();
                    let avail_width = ui.content_region_avail()[0];
                    let version_text = env!("CARGO_PKG_VERSION");
                    let version_width = ui.calc_text_size(version_text)[0];
                    ui.dummy([avail_width - version_width, 0.0]);
                    ui.same_line();
                    ui.text_colored([0.5, 0.5, 0.5, 1.0], version_text);
                });

                // Status bar
                let window_size = window.inner_size();
                ui.set_cursor_pos([0.0, window_size.height as f32 / hidpi_factor as f32 - 25.0]);
                ui.child_window("Status")
                    .size([window_size.width as f32 / hidpi_factor as f32, 25.0])
                    .build(|| {
                        if state.loading {
                            // Show loading progress
                            ui.text_colored([1.0, 0.8, 0.3, 1.0],
                                format!("Loading... {:.0}% ({})", state.loading_progress, state.loading_total)
                            );
                        } else if let Some(ref msg) = state.status_message {
                            ui.text(msg);
                        } else if state.file_loaded {
                            ui.text(format!(
                                "Messages: {} | DBC: {} | Position: {}",
                                state.messages.len(),
                                if state.dbc_loaded { "Loaded" } else { "None" },
                                state.playback.position()
                            ));
                        } else {
                            ui.text("Open a CAN log file to begin (File > Open CAN Log...)");
                        }
                    });

                // Create a dockspace over the main viewport
                // This allows windows to be docked/rearranged within the main window
                // Windows can be dragged and docked to different areas, but stay within the app
                ui.dockspace_over_main_viewport();

                // Render windows - these will dock into the dockspace above
                // Windows can be rearranged by dragging their tabs/bars

                if state.show_messages {
                    state.message_list.render(&ui, &mut state.show_messages, state.playback.is_playing());
                }

                // Process incremental chart data loading (runs even when charts window is hidden)
                state.process_pending_signal_loads();

                if state.show_charts {
                    let current_time = state.playback.current_time();
                    ui.window("Charts")
                        .size([600.0, 350.0], Condition::FirstUseEver)
                        .position([400.0, 30.0], Condition::FirstUseEver)
                        .opened(&mut state.show_charts)
                        .build(|| {
                            state.charts.render(ui, current_time, state.playback.is_playing());
                        });

                    // Handle seek request from chart click
                    // All values from chart are relative offsets from current time
                    // Positive = forward, Negative = backward
                    if let Some(offset_secs) = state.charts.take_seek_request() {
                        if let Some(current) = state.playback.current_time() {
                            let new_time = current + chrono::Duration::milliseconds((offset_secs * 1000.0) as i64);
                            state.playback.seek_to_time(Some(new_time));
                            state.seek_triggered_ui_update = true;
                        }
                    }

                    // Handle timeline actions from charts (play/pause/step)
                    if let Some(timeline_action) = state.charts.take_timeline_action() {
                        use crate::ui::multi_graph::TimelineAction;
                        match timeline_action {
                            TimelineAction::Play => state.playback.play(),
                            TimelineAction::Pause => state.playback.pause(),
                            TimelineAction::StepBack => state.playback.step_back(),
                            TimelineAction::StepForward => state.playback.step_forward(),
                            TimelineAction::None => {}
                        }
                    }
                }

                // Hardware Manager with action handling
                if state.show_hardware_manager {
                    let action = state.hardware_manager.render(&ui, &mut state.show_hardware_manager);
                    match action {
                        LiveModeAction::Connect { interface, config } => {
                            info!("[DERMO] Connect button clicked! Interface: {}, Bitrate: {}, Listen only: {}", interface, config.bitrate, config.listen_only);

                            // Determine interface type
                            let interface_type = if interface.starts_with("mock://") {
                                info!("[DERMO] Interface type: Virtual (mock)");
                                InterfaceType::Virtual
                            } else {
                                info!("[DERMO] Interface type: Serial");
                                InterfaceType::Serial
                            };

                            // Connect to the CAN interface
                            info!("[DERMO] Calling can_collection.connect()...");
                            let result = rt.block_on(state.can_collection.connect(
                                &interface,
                                crate::hardware::can_interface::CanConfig {
                                    bitrate: config.bitrate,
                                    fd_mode: false,
                                    listen_only: config.listen_only,
                                },
                                interface_type,
                            ));

                            info!("[DERMO] Connect result: {:?}", result);
                            match result {
                                Ok(bus_id) => {
                                    info!("[DERMO] Connecting as Bus {} (status will update when ready)...", bus_id);
                                    state.status_message = Some(format!("Connecting to {} as Bus {}...", interface, bus_id));
                                    state.hardware_manager.state_mut().add_connected_interface(
                                        bus_id,
                                        interface.clone(),
                                        crate::hardware::can_manager::ConnectionStatus::Connecting,
                                    );
                                }
                                Err(e) => {
                                    error!("[DERMO] Connection FAILED: {}", e);
                                    state.status_message = Some(format!("Failed to connect: {}", e));
                                }
                            }
                        }
                        LiveModeAction::Disconnect => {
                            info!("Disconnect from all interfaces");
                            rt.block_on(state.can_collection.disconnect_all());
                            state.hardware_manager.state_mut().clear_connected_interfaces();
                            state.status_message = Some("Disconnected from all CAN interfaces".to_string());
                        }
                        LiveModeAction::DisconnectBus { bus_id } => {
                            info!("Disconnect Bus {}", bus_id);
                            match rt.block_on(state.can_collection.disconnect(bus_id)) {
                                Ok(()) => {
                                    state.hardware_manager.state_mut().remove_connected_interface(bus_id);
                                    state.status_message = Some(format!("Disconnected Bus {}", bus_id));
                                }
                                Err(e) => {
                                    state.status_message = Some(format!("Disconnect failed: {}", e));
                                }
                            }
                        }
                        LiveModeAction::DisconnectAll => {
                            info!("Disconnect all interfaces");
                            rt.block_on(state.can_collection.disconnect_all());
                            state.hardware_manager.state_mut().clear_connected_interfaces();
                            state.status_message = Some("Disconnected all interfaces".to_string());
                        }
                        LiveModeAction::SendMessage { id, data } => {
                            info!("Send message: 0x{:03X} {:?}", id, data);
                            let msg = CanMessage::new(0, id, data.into());
                            // Send to bus 0 by default (could add UI to select bus)
                            let _ = rt.block_on(state.can_collection.send_to_bus(0, msg));
                        }
                        LiveModeAction::StartRecording => {
                            info!("[DERMO] Recording started");
                            state.status_message = Some("Recording started".to_string());
                        }
                        LiveModeAction::StopRecording => {
                            let live_state = state.hardware_manager.state();
                            let msg_count = live_state.live_messages.len();
                            info!("[DERMO] Recording stopped - {} messages captured", msg_count);

                            if !live_state.live_messages.is_empty() {
                                // Convert live messages to CanMessage format and load into main state
                                let recorded_messages: Vec<CanMessage> = live_state.live_messages
                                    .iter()
                                    .map(|lm| CanMessage {
                                        timestamp: lm.timestamp,
                                        bus: lm.bus,
                                        id: lm.id,
                                        data: lm.data.clone().into(),
                                    })
                                    .collect();

                                // Load into main state
                                state.messages = recorded_messages.clone();
                                state.playback = PlaybackEngine::new(recorded_messages.clone());
                                state.message_list.set_messages(recorded_messages);
                                state.file_loaded = true;
                                state.initial_data_populated = false;

                                // Update charts time range based on recording
                                if let (Some(first), Some(last)) = (state.messages.first(), state.messages.last()) {
                                    state.charts.set_data_time_range(first.timestamp, last.timestamp);
                                }

                                // Pre-populate charts if DBC is loaded
                                if state.dbc_loaded {
                                    state.populate_chart_data();
                                }

                                info!("[DERMO] Loaded {} recorded messages into playback", state.messages.len());
                            }

                            state.status_message = Some(format!("Recording stopped - {} messages loaded into playback", msg_count));
                        }
                        LiveModeAction::SaveData => {
                            info!("[DERMO] Save data requested - {} messages", state.hardware_manager.state().live_messages.len());
                            // Save to CSV file
                            let live_state = state.hardware_manager.state();
                            if let Some(path) = crate::ui::FileDialogs::export_csv_file() {
                                match std::fs::File::create(&path) {
                                    Ok(mut file) => {
                                        use std::io::Write;
                                        // Write CSV header matching 130b.csv format
                                        let _ = writeln!(file, "time,addr,bus,data");
                                        // Use recording_start for accurate relative timestamps
                                        let start_time = live_state.recording_start;
                                        // Write messages with actual relative time (realtime)
                                        for msg in &live_state.live_messages {
                                            // Calculate relative time in seconds with microsecond precision
                                            let rel_time = if let Some(start) = start_time {
                                                (msg.timestamp - start).num_microseconds().unwrap_or(0) as f64 / 1_000_000.0
                                            } else {
                                                0.0
                                            };
                                            // Data as hex string with 0x prefix
                                            let data_hex = if msg.data.is_empty() {
                                                "0x".to_string()
                                            } else {
                                                format!("0x{}", msg.data.iter()
                                                    .map(|b| format!("{:02X}", b))
                                                    .collect::<String>())
                                            };
                                            let _ = writeln!(file, "{:.3},0x{:03X},{},{}",
                                                rel_time, msg.id, msg.bus, data_hex);
                                        }
                                        state.status_message = Some(format!("Saved {} messages to {}", live_state.live_messages.len(), path.display()));
                                        info!("[DERMO] Saved {} messages to {}", live_state.live_messages.len(), path.display());
                                    }
                                    Err(e) => {
                                        state.status_message = Some(format!("Failed to save: {}", e));
                                        error!("[DERMO] Failed to save: {}", e);
                                    }
                                }
                            }
                        }
                        LiveModeAction::None => {}
                    }
                }

                // Poll for CAN messages when we have interfaces (including Connecting) - needed so status
                // can update from Connecting to Connected. Also used by live state and plugins.
                let has_interfaces = !state.hardware_manager.state().connected_interfaces.is_empty();
                let live_messages = if state.show_live_messages || state.hardware_manager.state().is_active || has_interfaces {
                    rt.block_on(state.can_collection.get_messages())
                } else {
                    Vec::new()
                };

                // Update live messages from CAN manager
                if state.show_live_messages || state.hardware_manager.state().is_active || has_interfaces {
                    // Sync interface stats from CanManagerCollection
                    let stats = rt.block_on(state.can_collection.get_stats());
                    state.hardware_manager.state_mut().sync_interface_stats(&stats);

                    let live_state = state.hardware_manager.state_mut();
                    let is_recording = live_state.is_recording;

                    // Update live state with received messages - only add to buffer if recording
                    for msg in &live_messages {
                        // Only store messages if recording is active
                        if is_recording {
                            live_state.add_message(msg.message.id, msg.message.data.to_vec(), msg.message.bus);
                        }

                        // Always update statistics
                        live_state.stats.messages_received += 1;

                        // Update Messages panel with live data
                        state.message_list.update_message(&msg.message);

                        // Decode and add to charts if signals are charted
                        let decoded = state.signal_decoder.decode_message(&msg.message);
                        for signal in &decoded {
                            let key = format!("{}@bus{}", signal.name, msg.message.bus);
                            if state.charts.has_signal(&key) {
                                state.charts.add_point(&key, signal.physical_value, msg.timestamp);
                            }
                        }
                    }

                    if state.show_live_messages {
                        let live_state_ref = state.hardware_manager.state();
                        state.live_message_window.render(&ui, live_state_ref, &mut state.show_live_messages);
                    }
                }

                // Message Sender window
                if state.show_message_sender {
                    let is_connected = state.hardware_manager.state().is_active;
                    if let Some((id, data)) = state.message_sender.render(&ui, is_connected, &mut state.show_message_sender) {
                        info!("Send CAN message: 0x{:03X} {:?}", id, data);
                        // TODO: Actually send the message through the interface
                    }
                }

                // Plugins - render visible plugins and process queued sends
                let connected_buses: Vec<u8> = state.hardware_manager.state()
                    .connected_interfaces
                    .iter()
                    .filter(|i| matches!(i.status, hardware::can_manager::ConnectionStatus::Connected))
                    .map(|i| i.bus_id)
                    .collect();
                let connected_interfaces: Vec<(u8, String)> = state.hardware_manager.state()
                    .connected_interfaces
                    .iter()
                    .filter(|i| matches!(i.status, hardware::can_manager::ConnectionStatus::Connected))
                    .map(|i| (i.bus_id, i.interface_name.clone()))
                    .collect();
                let is_connected = state.hardware_manager.state().is_active;

                // Build combined message buffer for plugins: playback (when file loaded) + live
                // Include discovery sample so existing sensors are found on load/connect
                state.plugin_message_buffer.clear();
                if state.file_loaded {
                    let discovery = state.playback.get_discovery_sample(10_000);
                    for msg in discovery {
                        state.plugin_message_buffer.push(ManagerMessage {
                            message: msg.clone(),
                            timestamp: msg.timestamp,
                        });
                    }
                    let window = state.playback.get_window(
                        Duration::seconds(60),
                        Duration::seconds(1),
                    );
                    for msg in window {
                        state.plugin_message_buffer.push(ManagerMessage {
                            message: msg.clone(),
                            timestamp: msg.timestamp,
                        });
                    }
                }
                // Live: include accumulated messages for discovery when connected
                if has_interfaces {
                    let live_state = state.hardware_manager.state();
                    let discovery_count = 10_000.min(live_state.live_messages.len());
                    let start = live_state.live_messages.len().saturating_sub(discovery_count);
                    for lm in &live_state.live_messages[start..] {
                        state.plugin_message_buffer.push(ManagerMessage {
                            message: crate::core::CanMessage {
                                timestamp: lm.timestamp,
                                bus: lm.bus,
                                id: lm.id,
                                data: lm.data.clone().into(),
                            },
                            timestamp: lm.timestamp,
                        });
                    }
                }
                state.plugin_message_buffer.extend(live_messages.iter().cloned());
                let has_playback = state.file_loaded;

                let plugin_ids: Vec<String> = state.plugin_registry.plugins()
                    .map(|(id, _, _)| id.to_string())
                    .collect();
                for id in &plugin_ids {
                    if state.plugin_registry.is_visible(id) {
                        let mut ctx = PluginContext {
                            queue_send: &mut state.plugin_send_queue,
                            is_connected,
                            has_playback,
                            connected_buses: &connected_buses,
                            connected_interfaces: &connected_interfaces,
                        };
                        state.plugin_registry.render_plugin(id, &ui, &mut ctx, &state.plugin_message_buffer);
                    }
                }

                // Process plugin queued messages (e.g. rusEFI wideband ECU status)
                for (bus_id, msg) in state.plugin_send_queue.drain(..) {
                    // Log commands (not ECU status which is sent every 10ms)
                    if msg.id != 0x0EF50000 {
                        info!(
                            "[Plugins] Sending CAN 0x{:08X} to bus {} ({} bytes): {:02X?}",
                            msg.id,
                            bus_id,
                            msg.data.len(),
                            msg.data
                        );
                    }
                    if let Err(e) = rt.block_on(state.can_collection.send_to_bus(bus_id, msg.clone())) {
                        error!("[Plugins] Failed to send: {}", e);
                    } else {
                        // Show sent messages in message list (TX, different color)
                        state.message_list.add_sent_message(&msg);
                    }
                }

                // Message Statistics window
                if state.show_message_stats {
                    state.message_stats.render(&ui, &mut state.show_message_stats);
                }

                // Pattern Analyzer window
                if state.show_pattern_analyzer {
                    state.pattern_analyzer.render(&ui, &mut state.show_pattern_analyzer);
                }

                // Bit Visualizer window - update with message data
                if state.show_bit_visualizer {
                    // Selection: set focused quadrant when user selects from message list
                    if let Some(selected_msg) = state.message_list.selected_message() {
                        state.bit_visualizer.set_message(selected_msg.id, selected_msg.bus, &selected_msg.data);
                    }

                    // Playback: update ALL quadrants with latest data for their respective messages
                    for (id, bus) in state.bit_visualizer.quadrant_messages() {
                        if let Some(msg_state) = state.message_list.get_state(id, bus) {
                            state.bit_visualizer.update_message_data(id, bus, &msg_state.data);
                        }
                    }

                    // Get list of charted signals
                    let charted: Vec<String> = state.charts.get_charted_signals();
                    state.bit_visualizer.set_charted_signals(charted);

                    state.bit_visualizer.render(&ui, &mut state.dbc_file, &mut state.show_bit_visualizer);

                    // Check for chart toggle requests
                    if let Some(signal_name) = state.bit_visualizer.take_chart_toggle_request() {
                        use std::io::Write;
                        let mut f = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open("/tmp/can-viz-chart-debug.txt")
                            .ok();
                        if let Some(ref mut f) = f {
                            let _ = writeln!(f, "main.rs: received chart toggle request for: {}", signal_name);
                        }

                        let was_charted = state.charts.has_signal(&signal_name);
                        if let Some(ref mut f) = f { let _ = writeln!(f, "  was_charted: {}", was_charted); }

                        // signal_name is now a bus-aware key from bit visualizer ("name@busN")
                        state.charts.toggle_signal_by_name(&signal_name);
                        // If signal was newly added, populate its data
                        if !was_charted {
                            state.populate_chart_data_for_signal(&signal_name);
                        }
                    }

                    // Sync DBC changes to other components
                    state.signal_decoder.set_dbc(state.dbc_file.clone());
                }

                // Log window
                if state.show_log {
                    state.log_window.render(&ui, &mut state.show_log);
                }

                // Keyboard Shortcuts help window
                if state.show_shortcuts {
                    state.shortcut_manager.render_help(&ui, &mut state.show_shortcuts);
                }

                // Export Dialog
                if let Some(export_request) = state.export_dialog.render(&ui) {
                    if let Some(path) = FileDialogs::export_csv_file() {
                        if let Ok(mut file) = std::fs::File::create(&path) {
                            use std::io::Write;
                            let _ = writeln!(file, "time,addr,bus,data");
                            let first_ts = state.messages.first().map(|m| m.timestamp);
                            for msg in &state.messages {
                                let rel_time = first_ts
                                    .map(|t| (msg.timestamp - t).num_microseconds().unwrap_or(0) as f64 / 1_000_000.0)
                                    .unwrap_or(0.0);
                                let data_hex = if msg.data.is_empty() {
                                    "0x".to_string()
                                } else {
                                    format!("0x{}", msg.data.iter().map(|b| format!("{:02X}", b)).collect::<String>())
                                };
                                let _ = writeln!(file, "{:.6},0x{:03X},{},{}", rel_time, msg.id, msg.bus, data_hex);
                            }
                            state.status_message = Some(format!("Exported {} messages to {}", state.messages.len(), path.display()));
                            info!("Exported {} messages to {}", state.messages.len(), path.display());
                        } else {
                            state.status_message = Some("Failed to create export file".to_string());
                        }
                    }
                }

                // About Dialog
                state.about_dialog.render(&ui);

                // No loading overlay - user can interact with UI while loading (status bar shows progress)

                // Prepare and render
                platform.prepare_render(&ui, &window);
                let draw_data = imgui.render();

                // Clear the screen before rendering
                unsafe {
                    gl_clear.clear_color(0.1, 0.1, 0.1, 1.0); // Dark gray background
                    gl_clear.clear(glow::COLOR_BUFFER_BIT);
                }

                renderer.render(draw_data).expect("Rendering failed");

                surface.swap_buffers(&context).expect("Failed to swap buffers");
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                state.save_settings();
                window_target.exit();
            }
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                // Resize the GL surface to match the new window size
                if let (Some(w), Some(h)) = (
                    std::num::NonZeroU32::new(size.width),
                    std::num::NonZeroU32::new(size.height)
                ) {
                    unsafe {
                        surface.resize(&context, w, h);
                    }
                }
            }
            Event::WindowEvent { event: WindowEvent::ScaleFactorChanged { scale_factor, .. }, .. } => {
                // Update hidpi factor when moving between displays
                hidpi_factor = scale_factor;
            }
            _ => {}
        }

        platform.handle_event(imgui.io_mut(), &window, &event);
    }).expect("EventLoop error");
}
