use imgui::{Ui, Condition};
use winit::event::{KeyEvent, ElementState};
use winit::keyboard::{KeyCode, PhysicalKey};

/// Keyboard shortcut manager
pub struct ShortcutManager {
    shortcuts: Vec<Shortcut>,
}

#[derive(Clone)]
pub struct Shortcut {
    pub key: PhysicalKey,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub action: ShortcutAction,
    pub description: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ShortcutAction {
    OpenFile,
    LoadDbc,
    SaveDbc,
    Play,
    Pause,
    Stop,
    ToggleMessages,
    ToggleGraph,
    ToggleFullscreen,
    ExportCsv,
    ClearData,
    SeekForward,
    SeekBackward,
    SpeedUp,
    SpeedDown,
    Quit,
}

impl ShortcutManager {
    pub fn new() -> Self {
        let mut manager = Self {
            shortcuts: Vec::new(),
        };
        manager.register_defaults();
        manager
    }

    fn register_defaults(&mut self) {
        // File operations
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::KeyO),
            ctrl: true,
            shift: false,
            alt: false,
            action: ShortcutAction::OpenFile,
            description: "Open CAN Log".to_string(),
        });
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::KeyD),
            ctrl: true,
            shift: false,
            alt: false,
            action: ShortcutAction::LoadDbc,
            description: "Load DBC".to_string(),
        });
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::KeyS),
            ctrl: true,
            shift: false,
            alt: false,
            action: ShortcutAction::SaveDbc,
            description: "Save DBC".to_string(),
        });
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::KeyE),
            ctrl: true,
            shift: false,
            alt: false,
            action: ShortcutAction::ExportCsv,
            description: "Export to CSV".to_string(),
        });

        // Playback controls
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::Space),
            ctrl: false,
            shift: false,
            alt: false,
            action: ShortcutAction::Play,
            description: "Play/Pause".to_string(),
        });
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::Space),
            ctrl: true,
            shift: false,
            alt: false,
            action: ShortcutAction::Pause,
            description: "Pause".to_string(),
        });
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::Escape),
            ctrl: false,
            shift: false,
            alt: false,
            action: ShortcutAction::Stop,
            description: "Stop".to_string(),
        });

        // Navigation
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::ArrowRight),
            ctrl: false,
            shift: false,
            alt: false,
            action: ShortcutAction::SeekForward,
            description: "Seek Forward".to_string(),
        });
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::ArrowLeft),
            ctrl: false,
            shift: false,
            alt: false,
            action: ShortcutAction::SeekBackward,
            description: "Seek Backward".to_string(),
        });
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::Equal),
            ctrl: false,
            shift: false,
            alt: false,
            action: ShortcutAction::SpeedUp,
            description: "Speed Up".to_string(),
        });
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::Minus),
            ctrl: false,
            shift: false,
            alt: false,
            action: ShortcutAction::SpeedDown,
            description: "Speed Down".to_string(),
        });

        // View toggles
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::KeyM),
            ctrl: false,
            shift: false,
            alt: false,
            action: ShortcutAction::ToggleMessages,
            description: "Toggle Messages Window".to_string(),
        });
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::KeyG),
            ctrl: false,
            shift: false,
            alt: false,
            action: ShortcutAction::ToggleGraph,
            description: "Toggle Signal Graph".to_string(),
        });
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::F11),
            ctrl: false,
            shift: false,
            alt: false,
            action: ShortcutAction::ToggleFullscreen,
            description: "Toggle Fullscreen".to_string(),
        });

        // Other
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::Delete),
            ctrl: false,
            shift: false,
            alt: false,
            action: ShortcutAction::ClearData,
            description: "Clear Data".to_string(),
        });
        self.register(Shortcut {
            key: PhysicalKey::Code(KeyCode::KeyQ),
            ctrl: true,
            shift: false,
            alt: false,
            action: ShortcutAction::Quit,
            description: "Quit".to_string(),
        });
    }

    fn register(&mut self, shortcut: Shortcut) {
        self.shortcuts.push(shortcut);
    }

    /// Process a key event and return the matching action (if any)
    pub fn process_event(&self, event: &KeyEvent, ctrl: bool, shift: bool, alt: bool) -> Option<ShortcutAction> {
        if event.state != ElementState::Pressed {
            return None;
        }

        for shortcut in &self.shortcuts {
            if shortcut.key == event.physical_key &&
               shortcut.ctrl == ctrl &&
               shortcut.shift == shift &&
               shortcut.alt == alt {
                return Some(shortcut.action);
            }
        }

        None
    }

    /// Render a shortcuts help window
    pub fn render_help(&self, ui: &Ui, is_open: &mut bool) {
        ui.window("Keyboard Shortcuts")
            .size([350.0, 400.0], Condition::FirstUseEver)
            .position([500.0, 200.0], Condition::FirstUseEver)
            .opened(is_open)
            .build(|| {
                let mut current_category = String::new();

                for shortcut in &self.shortcuts {
                    let category = match shortcut.action {
                        ShortcutAction::OpenFile |
                        ShortcutAction::LoadDbc |
                        ShortcutAction::SaveDbc |
                        ShortcutAction::ExportCsv => "File Operations",
                        ShortcutAction::Play |
                        ShortcutAction::Pause |
                        ShortcutAction::Stop |
                        ShortcutAction::SeekForward |
                        ShortcutAction::SeekBackward |
                        ShortcutAction::SpeedUp |
                        ShortcutAction::SpeedDown => "Playback",
                        ShortcutAction::ToggleMessages |
                        ShortcutAction::ToggleGraph |
                        ShortcutAction::ToggleFullscreen => "View",
                        ShortcutAction::ClearData |
                        ShortcutAction::Quit => "General",
                    };

                    if category != current_category {
                        if !current_category.is_empty() {
                            ui.separator();
                        }
                        ui.text(category);
                        current_category = category.to_string();
                    }

                    let key_name = key_to_string(shortcut.key);
                    let mut shortcut_str = String::new();
                    if shortcut.ctrl {
                        shortcut_str.push_str("Ctrl+");
                    }
                    if shortcut.shift {
                        shortcut_str.push_str("Shift+");
                    }
                    if shortcut.alt {
                        shortcut_str.push_str("Alt+");
                    }
                    shortcut_str.push_str(&key_name);

                    ui.text(format!("  {:15} - {}", shortcut_str, shortcut.description));
                }
            });
    }
}

fn key_to_string(key: PhysicalKey) -> String {
    match key {
        PhysicalKey::Code(code) => match code {
            KeyCode::Space => "Space".to_string(),
            KeyCode::Escape => "Esc".to_string(),
            KeyCode::ArrowLeft => "←".to_string(),
            KeyCode::ArrowRight => "→".to_string(),
            KeyCode::ArrowUp => "↑".to_string(),
            KeyCode::ArrowDown => "↓".to_string(),
            KeyCode::Equal => "+".to_string(),
            KeyCode::Minus => "-".to_string(),
            KeyCode::Delete => "Del".to_string(),
            KeyCode::F11 => "F11".to_string(),
            KeyCode::KeyA => "A".to_string(),
            KeyCode::KeyB => "B".to_string(),
            KeyCode::KeyC => "C".to_string(),
            KeyCode::KeyD => "D".to_string(),
            KeyCode::KeyE => "E".to_string(),
            KeyCode::KeyF => "F".to_string(),
            KeyCode::KeyG => "G".to_string(),
            KeyCode::KeyH => "H".to_string(),
            KeyCode::KeyI => "I".to_string(),
            KeyCode::KeyJ => "J".to_string(),
            KeyCode::KeyK => "K".to_string(),
            KeyCode::KeyL => "L".to_string(),
            KeyCode::KeyM => "M".to_string(),
            KeyCode::KeyN => "N".to_string(),
            KeyCode::KeyO => "O".to_string(),
            KeyCode::KeyP => "P".to_string(),
            KeyCode::KeyQ => "Q".to_string(),
            KeyCode::KeyR => "R".to_string(),
            KeyCode::KeyS => "S".to_string(),
            KeyCode::KeyT => "T".to_string(),
            KeyCode::KeyU => "U".to_string(),
            KeyCode::KeyV => "V".to_string(),
            KeyCode::KeyW => "W".to_string(),
            KeyCode::KeyX => "X".to_string(),
            KeyCode::KeyY => "Y".to_string(),
            KeyCode::KeyZ => "Z".to_string(),
            _ => format!("{:?}", code),
        },
        _ => "?".to_string(),
    }
}

impl Default for ShortcutManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Export functionality
pub struct ExportDialog {
    show: bool,
    export_type: ExportType,
    include_timestamps: bool,
    include_decoded: bool,
    status: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ExportType {
    Csv,
    Json,
    Log,
}

impl ExportDialog {
    pub fn new() -> Self {
        Self {
            show: false,
            export_type: ExportType::Csv,
            include_timestamps: true,
            include_decoded: false,
            status: None,
        }
    }

    pub fn show(&mut self) {
        self.show = true;
        self.status = None;
    }

    pub fn render(&mut self, ui: &Ui) -> Option<ExportRequest> {
        if !self.show {
            return None;
        }

        let mut result = None;

        ui.window("Export Data")
            .size([400.0, 250.0], Condition::FirstUseEver)
            .build(|| {
                ui.text("Export CAN Log Data");
                ui.separator();

                // Export type - use integer for radio buttons
                ui.text("Format:");
                let mut export_val = self.export_type as i32;
                if ui.radio_button("CSV", &mut export_val, ExportType::Csv as i32) {
                    self.export_type = ExportType::Csv;
                }
                if ui.radio_button("JSON", &mut export_val, ExportType::Json as i32) {
                    self.export_type = ExportType::Json;
                }
                if ui.radio_button("LOG", &mut export_val, ExportType::Log as i32) {
                    self.export_type = ExportType::Log;
                }

                ui.separator();

                // Options
                ui.checkbox("Include Timestamps", &mut self.include_timestamps);
                ui.checkbox("Include Decoded Signals", &mut self.include_decoded);
                if ui.is_item_hovered() {
                    ui.tooltip(|| {
                        ui.text("Requires DBC to be loaded");
                    });
                }

                ui.separator();

                // Status
                if let Some(ref status) = self.status {
                    ui.text(status);
                }

                // Buttons
                if ui.button("Export") {
                    result = Some(ExportRequest {
                        export_type: self.export_type,
                        include_timestamps: self.include_timestamps,
                        include_decoded: self.include_decoded,
                    });
                }
                ui.same_line();
                if ui.button("Cancel") {
                    self.show = false;
                }
            });

        result
    }
}

impl Default for ExportDialog {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct ExportRequest {
    pub export_type: ExportType,
    pub include_timestamps: bool,
    pub include_decoded: bool,
}

/// About dialog
pub struct AboutDialog {
    show: bool,
}

impl AboutDialog {
    pub fn new() -> Self {
        Self { show: false }
    }

    pub fn show(&mut self) {
        self.show = true;
    }

    pub fn render(&mut self, ui: &Ui) {
        if !self.show {
            return;
        }

        ui.window("About DERMO")
            .size([400.0, 300.0], Condition::FirstUseEver)
            .build(|| {
                ui.text("DERMO");
                ui.text_colored([0.7, 0.7, 0.7, 1.0], "Version 0.1.0");
                ui.separator();
                ui.text("DERMO - CAN signal viewer and analyzer");
                ui.text("A cross-platform CAN bus visualization tool");
                ui.text("similar to comma.ai's Cabana.");
                ui.separator();
                ui.text("Features:");
                ui.bullet_text("CAN log playback and visualization");
                ui.bullet_text("DBC file loading and editing");
                ui.bullet_text("Multi-signal graphing");
                ui.bullet_text("Timeline scrubbing");
                ui.bullet_text("USB-CAN interface support");
                ui.separator();
                ui.text("Built with Rust, ImGui, and Glow");
                ui.separator();
                if ui.button("Close") {
                    self.show = false;
                }
            });
    }
}

impl Default for AboutDialog {
    fn default() -> Self {
        Self::new()
    }
}
