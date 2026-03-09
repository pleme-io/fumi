//! MCP server for fumi chat client automation.
//!
//! Tools:
//!   `status`          — connection status for all protocols
//!   `version`         — server version info
//!   `config_get`      — get a config value by key
//!   `config_set`      — set a config value
//!   `list_channels`   — list accessible channels across protocols
//!   `send_message`    — send a message to a channel
//!   `get_history`     — read message history from a channel
//!   `list_servers`    — list connected servers/workspaces
//!   `switch_channel`  — switch the active channel

use kaname::rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;
use serde_json::json;

// ── Tool input types ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ConfigGetInput {
    #[schemars(description = "Config key to retrieve (e.g. 'behavior.notifications', 'voice.input_device').")]
    key: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ConfigSetInput {
    #[schemars(description = "Config key to set.")]
    key: String,
    #[schemars(description = "New value as a JSON string.")]
    value: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ListChannelsInput {
    #[schemars(description = "Filter by protocol: 'discord', 'matrix', or 'slack'. Omit for all.")]
    protocol: Option<String>,
    #[schemars(description = "Filter channel names containing this string.")]
    filter: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SendMessageInput {
    #[schemars(description = "Channel ID or name to send the message to.")]
    channel: String,
    #[schemars(description = "Message content (supports markdown).")]
    content: String,
    #[schemars(description = "Protocol to use: 'discord', 'matrix', or 'slack'. Required if channel name is ambiguous.")]
    protocol: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct GetHistoryInput {
    #[schemars(description = "Channel ID or name to read history from.")]
    channel: String,
    #[schemars(description = "Maximum number of messages to return (default 25).")]
    limit: Option<usize>,
    #[schemars(description = "Protocol to use if channel name is ambiguous.")]
    protocol: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SwitchChannelInput {
    #[schemars(description = "Channel ID or name to switch to.")]
    channel: String,
    #[schemars(description = "Protocol to use if channel name is ambiguous.")]
    protocol: Option<String>,
}

// ── MCP Server ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct FumiMcp {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl FumiMcp {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    // ── Standard tools ──────────────────────────────────────────────────────

    #[tool(description = "Get connection status for all configured protocols (Discord, Matrix, Slack).")]
    async fn status(&self) -> String {
        // TODO: query daemon via tsunagu IPC
        serde_json::to_string(&json!({
            "protocols": {
                "discord": { "connected": false, "accounts": 0 },
                "matrix": { "connected": false, "accounts": 0 },
                "slack": { "connected": false, "accounts": 0 }
            },
            "daemon_running": false,
            "total_unread": 0
        }))
        .unwrap_or_default()
    }

    #[tool(description = "Get fumi version information.")]
    async fn version(&self) -> String {
        serde_json::to_string(&json!({
            "name": "fumi",
            "version": env!("CARGO_PKG_VERSION"),
            "protocols": ["discord", "matrix", "slack"]
        }))
        .unwrap_or_default()
    }

    #[tool(description = "Get a configuration value by key.")]
    async fn config_get(&self, Parameters(input): Parameters<ConfigGetInput>) -> String {
        // TODO: read from shikumi ConfigStore
        serde_json::to_string(&json!({
            "key": input.key,
            "value": null,
            "error": "config store not connected (daemon not running)"
        }))
        .unwrap_or_default()
    }

    #[tool(description = "Set a configuration value. Changes are applied immediately via hot-reload.")]
    async fn config_set(&self, Parameters(input): Parameters<ConfigSetInput>) -> String {
        // TODO: write to shikumi ConfigStore
        serde_json::to_string(&json!({
            "key": input.key,
            "value": input.value,
            "applied": false,
            "error": "config store not connected (daemon not running)"
        }))
        .unwrap_or_default()
    }

    // ── Chat tools ──────────────────────────────────────────────────────────

    #[tool(description = "List accessible channels across all connected protocols. Optionally filter by protocol or name.")]
    async fn list_channels(&self, Parameters(input): Parameters<ListChannelsInput>) -> String {
        // TODO: query unified store via tsunagu IPC
        serde_json::to_string(&json!({
            "protocol_filter": input.protocol,
            "name_filter": input.filter,
            "channels": [],
            "total": 0
        }))
        .unwrap_or_default()
    }

    #[tool(description = "Send a message to a channel. Supports markdown formatting.")]
    async fn send_message(&self, Parameters(input): Parameters<SendMessageInput>) -> String {
        // TODO: send via appropriate backend through tsunagu IPC
        serde_json::to_string(&json!({
            "ok": false,
            "channel": input.channel,
            "protocol": input.protocol,
            "error": "daemon not running"
        }))
        .unwrap_or_default()
    }

    #[tool(description = "Read message history from a channel. Returns messages in chronological order.")]
    async fn get_history(&self, Parameters(input): Parameters<GetHistoryInput>) -> String {
        let limit = input.limit.unwrap_or(25);
        // TODO: fetch history via backend through tsunagu IPC
        serde_json::to_string(&json!({
            "channel": input.channel,
            "protocol": input.protocol,
            "limit": limit,
            "messages": [],
            "total": 0
        }))
        .unwrap_or_default()
    }

    #[tool(description = "List all connected servers and workspaces across protocols.")]
    async fn list_servers(&self) -> String {
        // TODO: query unified store via tsunagu IPC
        serde_json::to_string(&json!({
            "servers": [],
            "total": 0
        }))
        .unwrap_or_default()
    }

    #[tool(description = "Switch the active channel in the UI. If running in GUI mode, the view updates immediately.")]
    async fn switch_channel(&self, Parameters(input): Parameters<SwitchChannelInput>) -> String {
        // TODO: send switch command via tsunagu IPC
        serde_json::to_string(&json!({
            "ok": false,
            "channel": input.channel,
            "protocol": input.protocol,
            "error": "daemon not running"
        }))
        .unwrap_or_default()
    }
}

#[tool_handler]
impl ServerHandler for FumiMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Fumi multi-protocol chat client — send messages, read history, manage channels across Discord, Matrix, and Slack."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let server = FumiMcp::new().serve(stdio()).await?;
    server.waiting().await?;
    Ok(())
}
