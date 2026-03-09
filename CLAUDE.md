# Fumi (文) -- GPU Multi-Protocol Chat Client

Unified GPU-rendered chat client for Discord, Matrix, and Slack. Three protocols,
one interface, vim-modal, MCP-drivable, Rhai-scriptable.

## Build & Test

```bash
cargo build
cargo run                 # GUI mode
cargo run -- daemon       # background mode (persistent connections)
cargo run -- mcp          # MCP server (stdio)
cargo test --lib          # unit tests
RUST_LOG=debug cargo run  # with tracing
```

Nix: `nix build`, `nix run`, `nix develop`

## Competitive Position

| Competitor | Weakness fumi addresses |
|-----------|------------------------|
| Element (Electron) | GPU-rendered, not Electron; multi-protocol, not Matrix-only |
| Discord desktop (Electron) | GPU-rendered, not Electron; multi-protocol, not proprietary-only |
| Weechat (C, TUI) | GPU rendering, modern protocols (Matrix E2E), Rhai not Perl/Python |
| Beeper (cloud bridges) | Self-hosted, no cloud dependency, native performance |
| Slack desktop (Electron) | GPU-rendered, not Electron; unified inbox across protocols |

Unique: three protocols in one GPU app, vim-modal, MCP automation, Rhai scripting.

## Architecture

### Data Flow

```
Discord Gateway (serenity) --+
Matrix Sync (matrix-sdk)  ---+--> ChatBackend trait --> UnifiedStore
Slack Socket Mode (WS)    --+         |                    |
                                      v                    v
                               ChatEvent stream     egaku widget state
                                      |                    |
                                      v                    v
                               tsuuchi notify        garasu GPU render
                                      |                    |
                                      v                    v
                               daemon mode           mojiban rich text
                              (tsunagu IPC)          (markdown + embeds)
```

### Module Map

| Module | Purpose | Key Types |
|--------|---------|-----------|
| `protocol.rs` | Common trait + data model | `ChatBackend`, `Message`, `User`, `Channel`, `Server`, `ChatEvent`, `ChatError` |
| `discord.rs` | serenity gateway + REST + voice | `DiscordBackend` (impl ChatBackend) |
| `matrix.rs` | matrix-sdk sync + E2E + rooms | `MatrixBackend` (impl ChatBackend) |
| `slack.rs` | REST (todoku) + WebSocket events | `SlackBackend` (impl ChatBackend) |
| `render.rs` | GPU UI via garasu/egaku/mojiban | `ChatRenderer`, layout panels |
| `config.rs` | shikumi multi-account config | `FumiConfig`, `AccountConfig`, `ProtocolConfig` |
| `daemon.rs` | Background service via tsunagu | `FumiDaemon`, persistent connections |
| `mcp.rs` | MCP server via kaname | Tools: send, list, search, voice |
| `scripting.rs` | Rhai engine via soushi | `fumi.*` API bindings |
| `voice.rs` | Voice channels via oto | `VoiceSession`, mute/deafen state |

### Protocol Abstraction

The `ChatBackend` trait is the central abstraction. Every protocol backend maps its
native API to these common operations:

```rust
pub trait ChatBackend: Send + Sync {
    fn connect(&mut self) -> impl Future<Output = Result<(), ChatError>> + Send;
    fn disconnect(&mut self) -> impl Future<Output = Result<(), ChatError>> + Send;
    fn servers(&self) -> &[Server];
    fn send_message(&self, channel_id: &str, content: &str) -> ...;
    fn fetch_messages(&self, channel_id: &str, limit: usize, before: Option<&str>) -> ...;
    fn events(&self) -> broadcast::Receiver<ChatEvent>;
    fn protocol(&self) -> Protocol;
}
```

New protocols (XMPP, IRC, hiroba) are added by implementing this trait.

### Protocol Backends

**Discord** (serenity 0.12):
- Gateway for real-time events, REST for history/actions
- Voice via oto (join/leave/mute/deafen)
- Rich embeds, reactions, threads, forum posts
- Bot token or OAuth2 user token

**Matrix** (matrix-sdk 0.9):
- Sync loop for real-time events
- E2E encryption via vodozemac (built into matrix-sdk)
- Rooms, spaces, threads
- VoIP (future -- via oto when matrix-sdk adds VoIP support)

**Slack** (custom REST + WebSocket):
- No official Rust SDK -- use todoku HTTP client + tokio-tungstenite
- Socket Mode for real-time events
- REST API for channels, messages, threads, reactions
- Blocks rendering (future)

### Unified Store

All protocol data flows through a `UnifiedStore` that:
- Normalizes protocol-specific data into common types (Message, Channel, User)
- Maintains a merged timeline across protocols
- Tracks unread counts, mention counts per channel
- Provides the data source for egaku widgets

### GUI Layout

```
+-------------+---------------------------+-------------------+
| Servers     | Channel Messages          | Member List       |
|  [Discord]  |  [alice] hello world      |  alice (online)   |
|  [Matrix]   |  [bob]   hey there        |  bob (idle)       |
|  [Slack]    |  [carol] check this out   |  carol (dnd)      |
|-------------|                           |                   |
| Channels    |                           |                   |
|  #general   |                           |                   |
|  #dev       |                           |                   |
|  @alice     |                           |                   |
+-------------+---------------------------+-------------------+
| Mode: NORMAL | #general (Discord) | 3 unread | typing: bob  |
+-------------+---------------------------+-------------------+
```

Three-column layout. Left sidebar: server list + channel tree. Center: message
timeline with rich text (mojiban). Right sidebar: member list with presence.
Status bar shows mode, active channel, unread count, typing indicators.

### Threading Model

```
Main thread:   madori event loop -> winit -> GPU render
IO thread:     tokio runtime -> protocol backends -> ChatEvent stream
Daemon thread: tsunagu IPC -> GUI <-> daemon communication
```

In daemon mode, the IO thread runs standalone (no GUI). The daemon maintains
persistent WebSocket/gateway connections and relays events via tsunagu Unix socket.
GUI connects to the daemon for events and sends actions back.

## Dependency Migration Notice

**Cargo.toml currently uses old library names that need updating:**

| Current (old) | Correct (new) | Notes |
|---------------|--------------|-------|
| `fude` | `mojiban` | Rich text library, renamed |
| `hikidashi` | `hasami` | Clipboard library, renamed |
| `kotoba` | `kaname` | MCP server framework, renamed |

Update these git URLs when migrating to crates.io published versions.

## Shared Library Integration

| Library | Used For |
|---------|----------|
| **garasu** | `GpuContext`, `TextRenderer`, `ShaderPipeline` for GPU rendering |
| **madori** | `App::builder()`, event loop, render callback, input dispatch |
| **egaku** | `TextInput`, `ListView`, `TabBar`, `SplitPane`, `FocusManager`, `Theme` |
| **irodzuki** | Base16 theme -> GPU uniforms, ANSI palette |
| **mojiban** | Markdown -> styled spans for chat messages, code blocks |
| **oto** | Voice channel audio: capture, playback, mute/deafen |
| **todoku** | HTTP client for Slack REST API |
| **tsunagu** | Daemon mode: PID lifecycle, Unix socket IPC |
| **tsuuchi** | Desktop notifications for mentions, DMs |
| **hasami** | Clipboard: copy message text, paste into input |
| **shikumi** | Config discovery + hot-reload |
| **kaname** | MCP server framework (stdio transport) |
| **soushi** | Rhai scripting engine |
| **awase** | Hotkey system (modal vim bindings) |

## Configuration (shikumi)

File: `~/.config/fumi/fumi.yaml`
Env override: `$FUMI_CONFIG`
Env prefix: `FUMI_`
Hot-reload: ArcSwap + file watcher (symlink-aware for Nix)

```yaml
# Multi-account configuration
accounts:
  personal-discord:
    protocol: discord
    token_command: "cat /run/secrets/discord-token"
  work-slack:
    protocol: slack
    token_command: "cat /run/secrets/slack-token"
    workspace: "my-company"
  matrix:
    protocol: matrix
    homeserver: "https://matrix.org"
    username: "@user:matrix.org"
    password_command: "cat /run/secrets/matrix-password"

# Appearance
theme:
  name: nord
  custom_colors: {}

# Behavior
behavior:
  notifications: true
  notification_filter: mentions  # all | mentions | none
  daemon_mode: true
  startup_channels: ["personal-discord/#general"]

# Voice
voice:
  input_device: default
  output_device: default
  noise_suppression: true
  push_to_talk: false
  push_to_talk_key: "Space"

# Keybindings (see Hotkey System section)
keybindings: {}
```

## Hotkey System (awase)

Three modes: Normal, Insert, Command.

### Normal Mode (default)
| Key | Action |
|-----|--------|
| `j` / `k` | Scroll messages up/down |
| `J` / `K` | Previous/next channel |
| `h` / `l` | Collapse/expand sidebar |
| `Enter` | Reply to selected message |
| `i` | Enter Insert mode (focus input) |
| `:` | Enter Command mode |
| `/` | Search messages |
| `r` | React to message (opens emoji picker) |
| `t` | Open thread for selected message |
| `v` | Toggle voice channel |
| `Tab` | Cycle focus: servers -> channels -> messages -> members |
| `g g` | Jump to top of message list |
| `G` | Jump to bottom (latest message) |
| `Ctrl-u` / `Ctrl-d` | Half-page scroll |
| `y` | Yank (copy) message text |
| `p` | Paste from clipboard |

### Insert Mode
| Key | Action |
|-----|--------|
| `Esc` | Return to Normal mode |
| `Enter` | Send message |
| `Shift-Enter` | Newline in message |
| `@` | Trigger mention completion |
| `#` | Trigger channel completion |
| `:` | Trigger emoji completion |
| `Ctrl-a` | Attach file |
| `Tab` | Accept completion |

### Command Mode
| Command | Action |
|---------|--------|
| `:join #channel` | Join channel |
| `:leave` | Leave current channel |
| `:msg @user text` | Direct message |
| `:search query` | Search messages |
| `:status text` | Set status message |
| `:switch discord\|matrix\|slack` | Switch protocol view |
| `:mute` / `:unmute` | Toggle voice mute |
| `:deafen` / `:undeafen` | Toggle voice deafen |
| `:quit` / `:q` | Quit fumi |

## MCP Server (kaname)

Stdio transport. Tools for AI-driven chat automation.

| Tool | Parameters | Description |
|------|-----------|-------------|
| `send_message` | channel, content, [protocol] | Send a message |
| `list_channels` | [protocol], [filter] | List accessible channels |
| `list_users` | channel | List users in a channel |
| `read_messages` | channel, [limit], [before] | Read message history |
| `search_messages` | query, [channel], [protocol] | Search messages |
| `set_status` | text, [emoji] | Set user status |
| `join_channel` | channel, [protocol] | Join a channel |
| `leave_channel` | channel | Leave a channel |
| `create_channel` | name, [protocol], [type] | Create a channel |
| `get_unread` | [protocol] | Get unread counts |
| `voice_join` | channel | Join voice channel |
| `voice_leave` | | Leave voice channel |
| `status` | | Connection status for all protocols |
| `config_get` | key | Get config value |
| `config_set` | key, value | Set config value |

## Rhai Scripting (soushi)

Scripts in `~/.config/fumi/scripts/*.rhai`. Hot-reload on file change.

### API

```rhai
// Messaging
fumi.send(channel, text)       // Send message to channel
fumi.reply(msg_id, text)       // Reply to a message
fumi.react(msg_id, emoji)      // Add reaction
fumi.thread(msg_id, text)      // Reply in thread

// Navigation
fumi.channels()                // List channels
fumi.switch(channel)           // Switch to channel
fumi.search(query)             // Search messages

// Presence
fumi.status(text)              // Set status
fumi.status(text, emoji)       // Set status with emoji

// Voice
fumi.voice_join(channel)       // Join voice
fumi.voice_leave()             // Leave voice
fumi.voice_mute()              // Toggle mute
fumi.voice_deafen()            // Toggle deafen

// Notifications
fumi.notify(title, body)       // Send desktop notification

// Event hooks
fn on_startup() { ... }
fn on_shutdown() { ... }
fn on_message(msg) { ... }     // Called for every received message
fn on_mention(msg) { ... }     // Called when user is mentioned
fn on_dm(msg) { ... }          // Called for direct messages
```

### Example Plugin

```rhai
// ~/.config/fumi/scripts/auto-respond.rhai
// Auto-respond to DMs when status is "away"

fn on_dm(msg) {
    if fumi.status() == "away" {
        fumi.reply(msg.id, "I'm currently away. I'll respond when I'm back.");
    }
}

fn on_mention(msg) {
    fumi.notify("Mention", msg.author + " mentioned you in " + msg.channel);
}
```

## Nix Integration

### flake.nix

```
packages.${system}.default  -- fumi binary
overlays.default            -- pkgs.fumi
homeManagerModules.default  -- blackmatter.components.fumi.*
devShells.${system}.default -- dev environment
```

### Home-Manager Module

`blackmatter.components.fumi`:
- `enable` -- install fumi
- `package` -- fumi package (default: pkgs.fumi)
- `settings` -- attrs -> `~/.config/fumi/fumi.yaml`
- `scripting.initScript` -- lines -> `~/.config/fumi/init.rhai`
- `scripting.extraScripts` -- attrsOf lines -> `~/.config/fumi/scripts/<name>.rhai`
- `daemon.enable` -- run fumi in daemon mode via launchd/systemd

### flake.nix TODO

Current flake is single-system (`aarch64-darwin`). Needs `forAllSystems` for
multi-platform support (see hikyaku flake.nix for reference).

## Implementation Roadmap

### Phase 1 -- Protocol Foundation (current)
- [x] `ChatBackend` trait with full type system
- [x] `Protocol`, `Message`, `User`, `Channel`, `Server` types
- [x] `ChatEvent` enum for real-time event stream
- [x] `ChatError` error types
- [x] Serde roundtrip tests for all types
- [ ] Discord backend: serenity gateway connection, channel list, message history
- [ ] Matrix backend: sync loop, room list, message history
- [ ] Slack backend: WebSocket connection, channel list, message history
- [ ] `UnifiedStore`: merge protocol data into common timeline

### Phase 2 -- GPU Interface
- [ ] madori app scaffold (window, event loop, render callback)
- [ ] egaku layout: three-column split pane
- [ ] Server sidebar: protocol icons, server list, channel tree
- [ ] Message list: author, timestamp, content with mojiban rich text
- [ ] Input bar: text input with markdown preview
- [ ] Member sidebar: user list with presence indicators
- [ ] Status bar: mode, channel, unread count, typing

### Phase 3 -- Interactive Features
- [ ] awase hotkey system: Normal/Insert/Command modes
- [ ] Message scrollback with virtual scrolling
- [ ] Mention/channel/emoji completion in Insert mode
- [ ] Reactions (emoji picker)
- [ ] Threads (inline thread view)
- [ ] File attachments (upload + preview)
- [ ] Inline image rendering (via garasu texture)
- [ ] URL preview (link unfurling)

### Phase 4 -- Voice & Daemon
- [ ] oto voice: Discord voice channel join/leave/mute/deafen
- [ ] Matrix VoIP (when matrix-sdk supports it)
- [ ] tsunagu daemon mode: persistent connections, IPC to GUI
- [ ] tsuuchi notifications: mentions, DMs, custom filters
- [ ] Background sync: maintain connections when GUI closed
- [ ] Reconnection: automatic reconnect with exponential backoff

### Phase 5 -- MCP & Scripting
- [ ] kaname MCP server with all listed tools
- [ ] soushi Rhai engine with fumi.* API
- [ ] Plugin manifest (plugin.toml)
- [ ] Hot-reload scripts
- [ ] Event hooks (on_message, on_mention, on_dm)

### Phase 6 -- Polish
- [ ] E2E encryption status indicators (Matrix)
- [ ] Message editing and deletion
- [ ] Slash commands (protocol-native)
- [ ] Custom emoji / sticker support
- [ ] irodzuki theming (base16 -> GPU uniforms)
- [ ] Accessibility: screen reader, high contrast, font scaling

## Design Decisions

### Why three protocols (not extensible plugin system)?
Discord, Matrix, and Slack cover 90%+ of developer chat. Each has fundamentally
different APIs (gateway vs sync vs socket-mode) that benefit from purpose-built
backends. The `ChatBackend` trait still allows adding protocols (hiroba, IRC,
XMPP) without changing the UI layer.

### Why serenity (not twilight)?
Serenity is more mature, has voice support, and better documentation. Twilight
is lower-level and more flexible but requires more boilerplate.

### Why custom Slack backend (not a crate)?
No mature Rust Slack SDK exists. The Slack API is simple enough (REST + WebSocket)
that a thin wrapper using todoku + tokio-tungstenite is cleaner than pulling in
a half-maintained crate.

### Why daemon mode?
Chat clients need persistent connections. Closing the GUI should not disconnect
from protocols. The daemon maintains all WebSocket/gateway connections and relays
events to the GUI via tsunagu Unix socket when the GUI reconnects.

### Why mojiban (not raw string rendering)?
Chat messages contain markdown, code blocks, mentions, emoji, and embeds. Mojiban
converts these to styled spans that garasu renders with correct formatting, colors,
and font weight. Without mojiban, every message would be plain text.

### Bold-as-bright in chat context
Not applicable -- fumi uses semantic colors from irodzuki, not ANSI color mapping.
Terminal escape codes are not present in chat protocols.
