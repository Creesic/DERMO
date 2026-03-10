//! Plugin system for extensible hardware and protocol support
//!
//! Plugins provide modular windows that can interact with CAN hardware,
//! decode protocol-specific messages, and extend DERMO functionality.
//!
//! See `rusefi_wideband_can_protocol.md` for the rusEFI wideband protocol.

mod bench;
mod gdi;
mod isotp;
mod uds_diag;
mod uds_security;
mod wideband;

pub use bench::RusefiBenchPlugin;
pub use gdi::RusefiGdiPlugin;
pub use uds_diag::UdsDiagPlugin;
pub use uds_security::UdsSecurityPlugin;
pub use wideband::RusefiWidebandPlugin;

use imgui::Ui;
use std::collections::HashMap;
use crate::hardware::can_manager::ManagerMessage;

/// Context provided to plugins for interacting with the application
pub struct PluginContext<'a> {
    /// Queue messages to send - pushed during render, processed after
    pub queue_send: &'a mut Vec<(u8, crate::core::CanMessage)>,
    /// Whether any CAN interface is connected
    pub is_connected: bool,
    /// Whether playback is active (CSV/log file loaded) - plugins can show data from playback
    pub has_playback: bool,
    /// List of connected bus IDs
    pub connected_buses: &'a [u8],
    /// Connected interfaces: (bus_id, interface_name)
    pub connected_interfaces: &'a [(u8, String)],
}

/// Trait for DERMO plugins
///
/// Plugins can render UI windows and interact with CAN hardware
/// through the provided PluginContext.
pub trait Plugin: Send {
    /// Unique identifier for the plugin
    fn id(&self) -> &str;

    /// Human-readable name
    fn name(&self) -> &str;

    /// Short description
    fn description(&self) -> &str;

    /// Render the plugin's UI window
    ///
    /// Called each frame when the plugin window is visible.
    /// `messages` are CAN messages received since last frame.
    fn render(
        &mut self,
        ui: &Ui,
        ctx: &mut PluginContext,
        messages: &[crate::hardware::can_manager::ManagerMessage],
        is_open: &mut bool,
    );
}

/// Registry of available plugins
pub struct PluginRegistry {
    plugins: HashMap<String, Box<dyn Plugin>>,
    visibility: HashMap<String, bool>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            plugins: HashMap::new(),
            visibility: HashMap::new(),
        };

        // Register built-in plugins
        registry.register(Box::new(RusefiBenchPlugin::new()));
        registry.register(Box::new(RusefiGdiPlugin::new()));
        registry.register(Box::new(UdsSecurityPlugin::new()));
        registry.register(Box::new(UdsDiagPlugin::new()));
        registry.register(Box::new(RusefiWidebandPlugin::new()));
        registry
    }

    /// Register a plugin
    pub fn register(&mut self, plugin: Box<dyn Plugin>) {
        let id = plugin.id().to_string();
        self.visibility.insert(id.clone(), false);
        self.plugins.insert(id, plugin);
    }

    /// Get all registered plugins
    pub fn plugins(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.plugins.iter().map(|(id, p)| (id.as_str(), p.name(), p.description()))
    }

    /// Render a plugin by ID (avoids trait object lifetime issues)
    pub fn render_plugin(
        &mut self,
        id: &str,
        ui: &Ui,
        ctx: &mut PluginContext,
        messages: &[ManagerMessage],
    ) -> bool {
        if let Some(plugin) = self.plugins.get_mut(id) {
            let mut is_open = true;
            plugin.render(ui, ctx, messages, &mut is_open);
            if !is_open {
                self.set_visible(id, false);
            }
            true
        } else {
            false
        }
    }

    /// Check if plugin window is visible
    pub fn is_visible(&self, id: &str) -> bool {
        self.visibility.get(id).copied().unwrap_or(false)
    }

    /// Set plugin window visibility
    pub fn set_visible(&mut self, id: &str, visible: bool) {
        self.visibility.insert(id.to_string(), visible);
    }

    /// Toggle plugin visibility
    pub fn toggle_visible(&mut self, id: &str) {
        let v = self.visibility.entry(id.to_string()).or_insert(false);
        *v = !*v;
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}
