//! In-app Log window showing recent tracing output.

use imgui::{Condition, Ui};

/// Log window that displays recent log lines from the tracing buffer.
pub struct LogWindow;

impl LogWindow {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&mut self, ui: &Ui, is_open: &mut bool) {
        ui.window("Log")
            .size([600.0, 400.0], Condition::FirstUseEver)
            .position([50.0, 400.0], Condition::FirstUseEver)
            .opened(is_open)
            .build(|| {
                self.render_content(ui);
            });
    }

    fn render_content(&mut self, ui: &Ui) {
        if let Some(path) = crate::logging::log_file_path() {
            ui.text_colored([0.6, 0.6, 0.6, 1.0], format!("Log file: {}", path.display()));
            ui.separator();
        }

        let buffer = crate::logging::log_buffer();
        let lines = match buffer.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => return,
        };

        ui.child_window("log_scroll")
            .border(true)
            .build(|| {
                for line in &lines {
                    ui.text_wrapped(line);
                }
            });
    }
}

impl Default for LogWindow {
    fn default() -> Self {
        Self::new()
    }
}
