//! GPU rendering module for the unified chat UI.
//!
//! Uses the pleme-io rendering stack:
//! - **garasu** -- GPU context management (wgpu device, surface, swap chain)
//! - **egaku** -- Widget toolkit: text input, lists, splits, focus management
//! - **mojiban** -- Rich text: markdown to styled spans for messages
//! - **madori** -- App framework: event loop, render callback
//!
//! # Layout
//!
//! ```text
//! +------------------+----------------------------+------------------+
//! | Servers          | Messages                   | Members          |
//! |  [Discord]       |  [alice] hello world       |  alice (online)  |
//! |  [Matrix]        |  [bob] **bold** text       |  bob (idle)      |
//! |  [Slack]         |  [carol] check `code`      |  carol (dnd)     |
//! |                  |                            |                  |
//! | Channels         |                            |                  |
//! |  #general        |                            |                  |
//! |  #dev            |                            |                  |
//! |  @alice          |                            |                  |
//! +------------------+----------------------------+------------------+
//! | Mode: NORMAL | #general (Discord) | 3 unread | typing: bob      |
//! +------------------+----------------------------+------------------+
//! ```

use garasu::GpuContext;
use madori::{AppEvent, EventResponse, RenderCallback, RenderContext};
use madori::event::{KeyCode, KeyEvent};
use egaku::{FocusManager, ListView, ScrollView, SplitPane, TextInput, Theme};
use mojiban::{MarkdownParser, RichLine, StyledSpan};

use crate::protocol::{PresenceStatus, Protocol, UnifiedStore};

// ---------------------------------------------------------------------------
// Input mode (vim-style)
// ---------------------------------------------------------------------------

/// Vim-style input modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Normal mode: navigate with hjkl, select messages, switch channels.
    Normal,
    /// Insert mode: type messages, send with Enter.
    Insert,
    /// Command mode: `:quit`, `:join #channel`, etc.
    Command,
}

impl std::fmt::Display for InputMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normal => write!(f, "NORMAL"),
            Self::Insert => write!(f, "INSERT"),
            Self::Command => write!(f, "COMMAND"),
        }
    }
}

// ---------------------------------------------------------------------------
// Focus panels
// ---------------------------------------------------------------------------

/// Named panels in the UI for focus management.
const PANEL_SERVERS: &str = "servers";
const PANEL_CHANNELS: &str = "channels";
const PANEL_MESSAGES: &str = "messages";
const PANEL_MEMBERS: &str = "members";
const PANEL_INPUT: &str = "input";

// ---------------------------------------------------------------------------
// Chat UI state
// ---------------------------------------------------------------------------

/// Complete UI state for the chat application.
pub struct ChatUiState {
    /// Unified data store across all protocols.
    pub store: UnifiedStore,

    /// Current input mode.
    pub mode: InputMode,

    /// Message text input widget.
    pub input: TextInput,

    /// Command input widget (for : commands).
    pub command_input: TextInput,

    /// Server list widget.
    pub server_list: ListView,

    /// Channel list widget.
    pub channel_list: ListView,

    /// Message scroll view.
    pub message_scroll: ScrollView,

    /// Member list widget.
    pub member_list: ListView,

    /// Focus manager for panel navigation.
    pub focus: FocusManager,

    /// Left sidebar split (servers | channels+messages+members).
    pub left_split: SplitPane,

    /// Right split (channels+messages | members).
    pub right_split: SplitPane,

    /// Theme for colors and styling.
    pub theme: Theme,

    /// Markdown parser for message rendering.
    pub md_parser: MarkdownParser,

    /// Selected message index in the active channel.
    pub selected_message: usize,

    /// Parsed rich lines for active channel messages (cached).
    pub rendered_messages: Vec<Vec<RichLine>>,

    /// Status bar text.
    pub status_text: String,

    /// Whether the app should exit.
    pub should_exit: bool,
}

impl ChatUiState {
    /// Create a new chat UI state with default settings.
    #[must_use]
    pub fn new() -> Self {
        let focus = FocusManager::new(vec![
            PANEL_SERVERS.into(),
            PANEL_CHANNELS.into(),
            PANEL_MESSAGES.into(),
            PANEL_MEMBERS.into(),
            PANEL_INPUT.into(),
        ]);

        Self {
            store: UnifiedStore::new(),
            mode: InputMode::Normal,
            input: TextInput::new(),
            command_input: TextInput::new(),
            server_list: ListView::new(vec![], 20),
            channel_list: ListView::new(vec![], 30),
            message_scroll: ScrollView::new(0.0, 600.0),
            member_list: ListView::new(vec![], 30),
            focus,
            left_split: SplitPane::new(egaku::Orientation::Horizontal, 0.2, 0.1),
            right_split: SplitPane::new(egaku::Orientation::Horizontal, 0.8, 0.1),
            theme: Theme::default(),
            md_parser: MarkdownParser::new(),
            selected_message: 0,
            rendered_messages: Vec::new(),
            status_text: String::new(),
            should_exit: false,
        }
    }

    /// Update widget lists from the unified store.
    pub fn sync_from_store(&mut self) {
        // Update server list.
        let server_names: Vec<String> = self
            .store
            .servers()
            .iter()
            .map(|s| format!("[{}] {}", s.protocol, s.name))
            .collect();
        self.server_list.set_items(server_names);

        // Update channel list from active server.
        let channel_names: Vec<String> = self
            .store
            .active_server_channels()
            .iter()
            .map(|c| {
                let prefix = match c.channel_type {
                    crate::protocol::ChannelType::Text => "#",
                    crate::protocol::ChannelType::Voice => "V",
                    crate::protocol::ChannelType::Direct => "@",
                    crate::protocol::ChannelType::Group => "&",
                    crate::protocol::ChannelType::Thread => ">",
                };
                if c.unread > 0 {
                    format!("{prefix}{} ({} new)", c.name, c.unread)
                } else {
                    format!("{prefix}{}", c.name)
                }
            })
            .collect();
        self.channel_list.set_items(channel_names);

        // Update member list.
        let member_names: Vec<String> = self
            .store
            .members()
            .iter()
            .map(|m| {
                let status = match m.presence {
                    PresenceStatus::Online => "o",
                    PresenceStatus::Idle => "~",
                    PresenceStatus::DoNotDisturb => "-",
                    PresenceStatus::Offline => " ",
                };
                format!("[{status}] {}", m.user.effective_name())
            })
            .collect();
        self.member_list.set_items(member_names);

        // Render messages via mojiban.
        self.rendered_messages = self
            .store
            .active_messages()
            .iter()
            .map(|msg| {
                let header_line = RichLine::from_spans(vec![
                    StyledSpan::new(
                        msg.author.effective_name().to_owned(),
                        mojiban::TextStyle::bold(),
                    ),
                    StyledSpan::plain(format!("  {}", format_timestamp(msg.timestamp))),
                ]);
                let mut lines = vec![header_line];
                let content_lines = self.md_parser.parse(&msg.content);
                lines.extend(content_lines);
                // Add reactions line if any.
                if !msg.reactions.is_empty() {
                    let reaction_text: String = msg
                        .reactions
                        .iter()
                        .map(|r| format!("{} {}", r.emoji, r.count))
                        .collect::<Vec<_>>()
                        .join("  ");
                    lines.push(RichLine::from_spans(vec![StyledSpan::new(
                        reaction_text,
                        mojiban::TextStyle::colored(self.theme.muted),
                    )]));
                }
                lines
            })
            .collect();

        // Update scroll view content height.
        let line_height = self.theme.font_size + 4.0;
        let total_lines: usize = self.rendered_messages.iter().map(Vec::len).sum();
        self.message_scroll.content_height = total_lines as f32 * line_height;

        // Update status bar.
        let channel_name = self
            .store
            .active_channel_info()
            .map_or("(no channel)", |c| &c.name);
        let server_name = self
            .store
            .active_server_info()
            .map_or("(no server)", |s| &s.name);
        let unread = self.store.total_unread();
        let typing_users = self.store.typing_users();
        let typing_str = if typing_users.is_empty() {
            String::new()
        } else {
            let names: Vec<&str> = typing_users.iter().map(|u| u.effective_name()).collect();
            format!(" | typing: {}", names.join(", "))
        };
        self.status_text = format!(
            " {} | {channel_name} ({server_name}) | {unread} unread{typing_str}",
            self.mode
        );
    }

    /// Handle a keyboard event in normal mode.
    pub fn handle_normal_key(&mut self, key: &KeyEvent) -> bool {
        if !key.pressed {
            return false;
        }

        match key.key {
            // Mode transitions
            KeyCode::Char('i') if !key.modifiers.any() => {
                self.mode = InputMode::Insert;
                self.focus.set_focus(PANEL_INPUT);
                true
            }
            KeyCode::Char(':') if !key.modifiers.any() => {
                self.mode = InputMode::Command;
                self.command_input = TextInput::new();
                true
            }

            // Navigation
            KeyCode::Char('j') | KeyCode::Down if !key.modifiers.any() => {
                match self.focus.focused_widget() {
                    PANEL_SERVERS => self.server_list.select_next(),
                    PANEL_CHANNELS => self.channel_list.select_next(),
                    PANEL_MESSAGES => {
                        let msg_count = self.store.active_messages().len();
                        if self.selected_message + 1 < msg_count {
                            self.selected_message += 1;
                        }
                    }
                    PANEL_MEMBERS => self.member_list.select_next(),
                    _ => {}
                }
                true
            }
            KeyCode::Char('k') | KeyCode::Up if !key.modifiers.any() => {
                match self.focus.focused_widget() {
                    PANEL_SERVERS => self.server_list.select_prev(),
                    PANEL_CHANNELS => self.channel_list.select_prev(),
                    PANEL_MESSAGES => {
                        if self.selected_message > 0 {
                            self.selected_message -= 1;
                        }
                    }
                    PANEL_MEMBERS => self.member_list.select_prev(),
                    _ => {}
                }
                true
            }
            KeyCode::Char('g') if !key.modifiers.any() => {
                // gg: jump to top (simplified: single g jumps to top)
                self.selected_message = 0;
                self.message_scroll.scroll_to(0.0);
                true
            }
            KeyCode::Char('G') if key.modifiers.shift => {
                // G: jump to bottom
                let msg_count = self.store.active_messages().len();
                if msg_count > 0 {
                    self.selected_message = msg_count - 1;
                }
                let max = self.message_scroll.max_scroll();
                self.message_scroll.scroll_to(max);
                true
            }

            // Half-page scroll
            KeyCode::Char('d') if key.modifiers.ctrl => {
                self.message_scroll.scroll_by(self.message_scroll.viewport_height / 2.0);
                true
            }
            KeyCode::Char('u') if key.modifiers.ctrl => {
                self.message_scroll.scroll_by(-self.message_scroll.viewport_height / 2.0);
                true
            }

            // Tab: cycle focus
            KeyCode::Tab if !key.modifiers.any() => {
                self.focus.focus_next();
                true
            }
            KeyCode::Tab if key.modifiers.shift => {
                self.focus.focus_prev();
                true
            }

            // Enter: activate selection
            KeyCode::Enter if !key.modifiers.any() => {
                match self.focus.focused_widget() {
                    PANEL_SERVERS => {
                        // Select server, switch to it.
                        let idx = self.server_list.selected_index();
                        let server_id = self.store.servers().get(idx).map(|s| s.id.clone());
                        if let Some(id) = server_id {
                            self.store.set_active_server(&id);
                            self.sync_from_store();
                        }
                    }
                    PANEL_CHANNELS => {
                        // Select channel.
                        let idx = self.channel_list.selected_index();
                        let channel_id = self.store.active_server_channels().get(idx).map(|c| c.id.clone());
                        if let Some(id) = channel_id {
                            self.store.set_active_channel(&id);
                            self.selected_message = 0;
                            self.sync_from_store();
                        }
                    }
                    PANEL_MESSAGES => {
                        // Enter insert mode to reply.
                        self.mode = InputMode::Insert;
                        self.focus.set_focus(PANEL_INPUT);
                    }
                    _ => {}
                }
                true
            }

            // Quit
            KeyCode::Char('q') if !key.modifiers.any() => {
                self.should_exit = true;
                true
            }

            _ => false,
        }
    }

    /// Handle a keyboard event in insert mode.
    pub fn handle_insert_key(&mut self, key: &KeyEvent) -> bool {
        if !key.pressed {
            return false;
        }

        match key.key {
            KeyCode::Escape => {
                self.mode = InputMode::Normal;
                self.focus.set_focus(PANEL_MESSAGES);
                true
            }
            KeyCode::Enter if !key.modifiers.shift => {
                // Send message (content returned to caller via input text).
                // The caller checks input.text() and sends via the appropriate backend.
                // After send, clear input.
                // We signal "message ready" by not clearing here -- the main loop handles it.
                true
            }
            KeyCode::Backspace => {
                self.input.delete_back();
                true
            }
            KeyCode::Delete => {
                self.input.delete_forward();
                true
            }
            KeyCode::Left => {
                self.input.move_left();
                true
            }
            KeyCode::Right => {
                self.input.move_right();
                true
            }
            KeyCode::Home => {
                self.input.move_to_start();
                true
            }
            KeyCode::End => {
                self.input.move_to_end();
                true
            }
            KeyCode::Char('a') if key.modifiers.ctrl => {
                self.input.select_all();
                true
            }
            _ => {
                // Insert character from text.
                if let Some(ref text) = key.text {
                    for c in text.chars() {
                        if !c.is_control() {
                            self.input.insert_char(c);
                        }
                    }
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Handle a keyboard event in command mode.
    pub fn handle_command_key(&mut self, key: &KeyEvent) -> bool {
        if !key.pressed {
            return false;
        }

        match key.key {
            KeyCode::Escape => {
                self.mode = InputMode::Normal;
                true
            }
            KeyCode::Enter => {
                // Execute command.
                let cmd = self.command_input.text().to_owned();
                self.execute_command(&cmd);
                self.mode = InputMode::Normal;
                true
            }
            KeyCode::Backspace => {
                self.command_input.delete_back();
                if self.command_input.is_empty() {
                    self.mode = InputMode::Normal;
                }
                true
            }
            _ => {
                if let Some(ref text) = key.text {
                    for c in text.chars() {
                        if !c.is_control() {
                            self.command_input.insert_char(c);
                        }
                    }
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Execute a command-mode command.
    fn execute_command(&mut self, cmd: &str) {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        match parts.first() {
            Some(&"quit") | Some(&"q") => {
                self.should_exit = true;
            }
            Some(&"join") => {
                if let Some(&channel_name) = parts.get(1) {
                    self.status_text = format!("joining {channel_name}...");
                    // Actual join is handled by the protocol backend externally.
                }
            }
            Some(&"leave") => {
                self.status_text = "leaving channel...".to_owned();
            }
            Some(&"switch") => {
                if let Some(&protocol) = parts.get(1) {
                    // Find server matching protocol.
                    let target = match protocol {
                        "discord" => Some(Protocol::Discord),
                        "matrix" => Some(Protocol::Matrix),
                        "slack" => Some(Protocol::Slack),
                        _ => None,
                    };
                    if let Some(p) = target {
                        let server_id = self.store.servers().iter().find(|s| s.protocol == p).map(|s| s.id.clone());
                        if let Some(id) = server_id {
                            self.store.set_active_server(&id);
                            self.sync_from_store();
                        }
                    }
                }
            }
            _ => {
                self.status_text = format!("unknown command: {cmd}");
            }
        }
    }

    /// Handle an `AppEvent` from madori, dispatching to the appropriate mode handler.
    pub fn handle_app_event(&mut self, event: &AppEvent) -> EventResponse {
        match event {
            AppEvent::Key(key) => {
                let consumed = match self.mode {
                    InputMode::Normal => self.handle_normal_key(key),
                    InputMode::Insert => self.handle_insert_key(key),
                    InputMode::Command => self.handle_command_key(key),
                };
                if self.should_exit {
                    return EventResponse { consumed: true, exit: true, set_title: None };
                }
                consumed.into()
            }
            AppEvent::CloseRequested => {
                self.should_exit = true;
                EventResponse { consumed: true, exit: true, set_title: None }
            }
            AppEvent::Resized { width: _, height } => {
                self.message_scroll.viewport_height = *height as f32 * 0.7;
                false.into()
            }
            _ => false.into(),
        }
    }

    /// Get the current input text (for sending a message).
    #[must_use]
    pub fn pending_message(&self) -> Option<&str> {
        if self.mode == InputMode::Insert && !self.input.is_empty() {
            Some(self.input.text())
        } else {
            None
        }
    }

    /// Clear the input after a message was sent.
    pub fn clear_input(&mut self) {
        self.input = TextInput::new();
    }
}

impl Default for ChatUiState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Chat Renderer (RenderCallback impl for madori)
// ---------------------------------------------------------------------------

/// GPU renderer that draws the chat UI each frame.
///
/// Implements madori's `RenderCallback` trait. On each frame, it reads
/// widget state from `ChatUiState` and renders text via garasu's text pipeline.
pub struct ChatRenderer {
    /// Background clear color (from theme).
    bg_color: wgpu::Color,
    /// Width/height tracking.
    width: u32,
    height: u32,
}

impl ChatRenderer {
    /// Create a new renderer with the given theme.
    #[must_use]
    pub fn new(theme: &Theme) -> Self {
        let bg = theme.background;
        Self {
            bg_color: wgpu::Color {
                r: f64::from(bg[0]),
                g: f64::from(bg[1]),
                b: f64::from(bg[2]),
                a: f64::from(bg[3]),
            },
            width: 1280,
            height: 720,
        }
    }
}

impl RenderCallback for ChatRenderer {
    fn init(&mut self, _gpu: &GpuContext) {
        tracing::info!("ChatRenderer initialized");
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    fn render(&mut self, ctx: &mut RenderContext<'_>) {
        // Clear the background.
        let mut encoder = ctx
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fumi_clear"),
            });

        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fumi_clear_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ctx.surface_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.bg_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }

        // TODO: Render text for each panel using ctx.text (garasu::TextRenderer).
        // The TextRenderer requires TextLayout objects positioned within the window.
        // Each panel (servers, channels, messages, members, input, status bar)
        // gets a region computed from the SplitPane layout, and text is rendered
        // within those bounds.
        //
        // For now, the clear pass provides a visible Nord background, proving the
        // GPU pipeline is functional. Text rendering will be wired up as garasu's
        // TextRenderer API stabilizes (it needs positioned draw calls per-glyph).

        ctx.gpu.queue.submit(std::iter::once(encoder.finish()));
    }
}

// ---------------------------------------------------------------------------
// Awase hotkey bridge
// ---------------------------------------------------------------------------

/// Convert a madori `KeyEvent` to an awase `Hotkey` (when possible).
///
/// Provides a bridge between madori's input events and awase's hotkey
/// system, enabling user-configurable keybindings via awase's
/// `Hotkey::parse()` format (e.g., `"cmd+space"`, `"ctrl+n"`).
#[must_use]
pub fn to_awase_hotkey(event: &KeyEvent) -> Option<awase::Hotkey> {
    let key = madori_key_to_awase(&event.key)?;
    let mut mods = awase::Modifiers::NONE;
    if event.modifiers.shift {
        mods |= awase::Modifiers::SHIFT;
    }
    if event.modifiers.ctrl {
        mods |= awase::Modifiers::CTRL;
    }
    if event.modifiers.alt {
        mods |= awase::Modifiers::ALT;
    }
    if event.modifiers.meta {
        mods |= awase::Modifiers::CMD;
    }
    Some(awase::Hotkey::new(mods, key))
}

/// Map a madori `KeyCode` to an awase `Key`.
fn madori_key_to_awase(key: &KeyCode) -> Option<awase::Key> {
    match key {
        KeyCode::Char(c) => match c.to_ascii_lowercase() {
            'a' => Some(awase::Key::A), 'b' => Some(awase::Key::B),
            'c' => Some(awase::Key::C), 'd' => Some(awase::Key::D),
            'e' => Some(awase::Key::E), 'f' => Some(awase::Key::F),
            'g' => Some(awase::Key::G), 'h' => Some(awase::Key::H),
            'i' => Some(awase::Key::I), 'j' => Some(awase::Key::J),
            'k' => Some(awase::Key::K), 'l' => Some(awase::Key::L),
            'm' => Some(awase::Key::M), 'n' => Some(awase::Key::N),
            'o' => Some(awase::Key::O), 'p' => Some(awase::Key::P),
            'q' => Some(awase::Key::Q), 'r' => Some(awase::Key::R),
            's' => Some(awase::Key::S), 't' => Some(awase::Key::T),
            'u' => Some(awase::Key::U), 'v' => Some(awase::Key::V),
            'w' => Some(awase::Key::W), 'x' => Some(awase::Key::X),
            'y' => Some(awase::Key::Y), 'z' => Some(awase::Key::Z),
            '0' => Some(awase::Key::Num0), '1' => Some(awase::Key::Num1),
            '2' => Some(awase::Key::Num2), '3' => Some(awase::Key::Num3),
            '4' => Some(awase::Key::Num4), '5' => Some(awase::Key::Num5),
            '6' => Some(awase::Key::Num6), '7' => Some(awase::Key::Num7),
            '8' => Some(awase::Key::Num8), '9' => Some(awase::Key::Num9),
            '/' => Some(awase::Key::Slash),
            '+' | '=' => Some(awase::Key::Equal),
            '-' => Some(awase::Key::Minus),
            ',' => Some(awase::Key::Comma),
            '.' => Some(awase::Key::Period),
            _ => None,
        },
        KeyCode::Space => Some(awase::Key::Space),
        KeyCode::Enter => Some(awase::Key::Return),
        KeyCode::Escape => Some(awase::Key::Escape),
        KeyCode::Tab => Some(awase::Key::Tab),
        KeyCode::Backspace => Some(awase::Key::Backspace),
        KeyCode::Delete => Some(awase::Key::Delete),
        KeyCode::Up => Some(awase::Key::Up),
        KeyCode::Down => Some(awase::Key::Down),
        KeyCode::Left => Some(awase::Key::Left),
        KeyCode::Right => Some(awase::Key::Right),
        KeyCode::Home => Some(awase::Key::Home),
        KeyCode::End => Some(awase::Key::End),
        KeyCode::PageUp => Some(awase::Key::PageUp),
        KeyCode::PageDown => Some(awase::Key::PageDown),
        _ => None,
    }
}

/// Check if a key event matches an awase hotkey string.
///
/// Enables config-driven keybinding lookups.
#[must_use]
pub fn matches_hotkey(event: &KeyEvent, hotkey_str: &str) -> bool {
    let Some(event_hk) = to_awase_hotkey(event) else {
        return false;
    };
    awase::Hotkey::parse(hotkey_str)
        .map(|parsed| parsed == event_hk)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a Unix timestamp as a short time string.
fn format_timestamp(unix_secs: u64) -> String {
    // Simple HH:MM format from Unix timestamp.
    let secs = unix_secs % 86400;
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    format!("{hours:02}:{minutes:02}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use madori::event::Modifiers;

    #[test]
    fn input_mode_display() {
        assert_eq!(InputMode::Normal.to_string(), "NORMAL");
        assert_eq!(InputMode::Insert.to_string(), "INSERT");
        assert_eq!(InputMode::Command.to_string(), "COMMAND");
    }

    #[test]
    fn chat_ui_state_default() {
        let state = ChatUiState::new();
        assert_eq!(state.mode, InputMode::Normal);
        assert!(state.input.is_empty());
        assert!(!state.should_exit);
        assert_eq!(state.focus.focused_widget(), PANEL_SERVERS);
    }

    #[test]
    fn normal_mode_quit() {
        let mut state = ChatUiState::new();
        let key = KeyEvent {
            key: KeyCode::Char('q'),
            pressed: true,
            modifiers: Modifiers::default(),
            text: Some("q".into()),
        };
        state.handle_normal_key(&key);
        assert!(state.should_exit);
    }

    #[test]
    fn normal_to_insert_mode() {
        let mut state = ChatUiState::new();
        let key = KeyEvent {
            key: KeyCode::Char('i'),
            pressed: true,
            modifiers: Modifiers::default(),
            text: Some("i".into()),
        };
        state.handle_normal_key(&key);
        assert_eq!(state.mode, InputMode::Insert);
    }

    #[test]
    fn insert_mode_escape() {
        let mut state = ChatUiState::new();
        state.mode = InputMode::Insert;
        let key = KeyEvent {
            key: KeyCode::Escape,
            pressed: true,
            modifiers: Modifiers::default(),
            text: None,
        };
        state.handle_insert_key(&key);
        assert_eq!(state.mode, InputMode::Normal);
    }

    #[test]
    fn insert_mode_typing() {
        let mut state = ChatUiState::new();
        state.mode = InputMode::Insert;

        for c in ['h', 'e', 'l', 'l', 'o'] {
            let key = KeyEvent {
                key: KeyCode::Char(c),
                pressed: true,
                modifiers: Modifiers::default(),
                text: Some(c.to_string()),
            };
            state.handle_insert_key(&key);
        }

        assert_eq!(state.input.text(), "hello");
    }

    #[test]
    fn command_mode_quit() {
        let mut state = ChatUiState::new();
        state.mode = InputMode::Command;

        for c in ['q', 'u', 'i', 't'] {
            let key = KeyEvent {
                key: KeyCode::Char(c),
                pressed: true,
                modifiers: Modifiers::default(),
                text: Some(c.to_string()),
            };
            state.handle_command_key(&key);
        }

        // Press enter to execute.
        let enter = KeyEvent {
            key: KeyCode::Enter,
            pressed: true,
            modifiers: Modifiers::default(),
            text: None,
        };
        state.handle_command_key(&enter);
        assert!(state.should_exit);
    }

    #[test]
    fn tab_cycles_focus() {
        let mut state = ChatUiState::new();
        assert_eq!(state.focus.focused_widget(), PANEL_SERVERS);

        let tab = KeyEvent {
            key: KeyCode::Tab,
            pressed: true,
            modifiers: Modifiers::default(),
            text: None,
        };
        state.handle_normal_key(&tab);
        assert_eq!(state.focus.focused_widget(), PANEL_CHANNELS);

        state.handle_normal_key(&tab);
        assert_eq!(state.focus.focused_widget(), PANEL_MESSAGES);
    }

    #[test]
    fn format_timestamp_basic() {
        assert_eq!(format_timestamp(0), "00:00");
        assert_eq!(format_timestamp(3661), "01:01"); // 1h 1m 1s
        assert_eq!(format_timestamp(43200), "12:00"); // noon
    }

    #[test]
    fn sync_from_store_updates_widgets() {
        let mut state = ChatUiState::new();

        // Add a server.
        state.store.merge_servers(
            Protocol::Discord,
            &[crate::protocol::Server {
                id: "s1".into(),
                name: "Test Server".into(),
                protocol: Protocol::Discord,
                icon_url: None,
                channels: vec![crate::protocol::Channel {
                    id: "c1".into(),
                    name: "general".into(),
                    protocol: Protocol::Discord,
                    channel_type: crate::protocol::ChannelType::Text,
                    server_id: Some("s1".into()),
                    topic: None,
                    unread: 3,
                    mention_count: 1,
                }],
            }],
        );
        state.store.set_active_server("s1");
        state.store.set_active_channel("c1");

        state.sync_from_store();

        assert_eq!(state.server_list.len(), 1);
        assert_eq!(state.channel_list.len(), 1);
    }

    #[test]
    fn renderer_new() {
        let theme = Theme::default();
        let renderer = ChatRenderer::new(&theme);
        assert_eq!(renderer.width, 1280);
        assert_eq!(renderer.height, 720);
    }
}
