//! Fumi (文) — GPU-rendered multi-protocol chat client.
//!
//! Unified chat interface for Discord, Matrix, and Slack:
//! - GPU-accelerated UI via garasu + egaku widgets
//! - Rich text (markdown, embeds, reactions) via mojiban
//! - Voice chat via oto
//! - Multi-protocol: one interface, many backends
//! - Hot-reloadable configuration via shikumi
//! - Background daemon mode via tsunagu

mod config;
mod daemon;
mod discord;
mod matrix;
mod mcp;
mod protocol;
mod render;
mod slack;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::FumiConfig;
use crate::protocol::{ChatBackend, Protocol};
use crate::render::{ChatRenderer, ChatUiState};

#[derive(Parser)]
#[command(name = "fumi", version, about = "GPU-rendered multi-protocol chat client")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Configuration file override
    #[arg(long, env = "FUMI_CONFIG")]
    config: Option<std::path::PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the GUI (default)
    Open,
    /// Start the background daemon (maintains connections)
    Daemon,
    /// List configured accounts and their connection status
    Accounts,
    /// Send a message from CLI
    Send {
        /// Protocol (discord, matrix, slack)
        #[arg(short, long)]
        protocol: String,
        /// Channel/room name or ID
        #[arg(short, long)]
        channel: String,
        /// Message text
        message: String,
    },
    /// Check daemon health status
    Health,
    /// Start MCP server (stdio transport)
    Mcp,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let config = config::load(&cli.config)?;

    match cli.command {
        None | Some(Commands::Open) => {
            run_gui(config)?;
        }
        Some(Commands::Daemon) => {
            run_daemon(config)?;
        }
        Some(Commands::Accounts) => {
            list_accounts(&config);
        }
        Some(Commands::Send {
            protocol,
            channel,
            message,
        }) => {
            send_message(&config, &protocol, &channel, &message)?;
        }
        Some(Commands::Health) => {
            check_health();
        }
        Some(Commands::Mcp) => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                mcp::run().await.map_err(|e| anyhow::anyhow!("MCP server error: {e}"))
            })?;
        }
    }

    Ok(())
}

/// Launch the GPU chat UI.
fn run_gui(config: FumiConfig) -> anyhow::Result<()> {
    tracing::info!("launching fumi GUI");

    let theme = egaku::Theme::default();
    let renderer = ChatRenderer::new(&theme);
    let mut ui_state = ChatUiState::new();

    // Build the tokio runtime for async protocol connections.
    let rt = tokio::runtime::Runtime::new()?;

    // Connect backends and populate initial data.
    rt.block_on(async {
        connect_all_backends(&config, &mut ui_state).await;
    });

    // Sync UI state from the store.
    ui_state.sync_from_store();

    // Run the madori app with our renderer and event handler.
    madori::App::builder(renderer)
        .title("fumi — chat")
        .size(1280, 720)
        .on_event(move |event: &madori::AppEvent, _renderer: &mut ChatRenderer| {
            let response = ui_state.handle_app_event(event);

            // Check if user wants to send a message.
            if let madori::AppEvent::Key(key) = event {
                if key.pressed
                    && key.key == madori::event::KeyCode::Enter
                    && ui_state.mode == render::InputMode::Insert
                    && !ui_state.input.is_empty()
                {
                    let msg_text = ui_state.input.text().to_owned();
                    if let Some(channel_id) = ui_state.store.active_channel() {
                        let channel_id = channel_id.to_owned();
                        tracing::info!(channel = %channel_id, "sending: {msg_text}");
                        // In a full implementation, we'd send via the appropriate backend here.
                        // For now, log the intent.
                    }
                    ui_state.clear_input();
                }
            }

            // Periodically sync from store on redraw.
            if matches!(event, madori::AppEvent::RedrawRequested) {
                ui_state.sync_from_store();
            }

            response
        })
        .run()
        .map_err(|e| anyhow::anyhow!("GUI error: {e}"))?;

    Ok(())
}

/// Run in daemon mode: maintain persistent connections.
fn run_daemon(config: FumiConfig) -> anyhow::Result<()> {
    tracing::info!("starting fumi daemon");

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let mut daemon = daemon::FumiDaemon::new(config);

        if daemon.is_running() {
            tracing::error!("fumi daemon is already running");
            return Err(anyhow::anyhow!("daemon already running"));
        }

        daemon.run().await
    })
}

/// List configured accounts.
fn list_accounts(config: &FumiConfig) {
    println!("Configured accounts:");
    println!();

    for (i, account) in config.accounts.discord.iter().enumerate() {
        let has_token = account.token.is_some() || account.token_command.is_some();
        let status = if has_token { "configured" } else { "no token" };
        println!("  Discord #{}: {} ({})", i + 1, account.label, status);
    }

    for (i, account) in config.accounts.matrix.iter().enumerate() {
        let has_auth = account.token.is_some()
            || account.token_command.is_some()
            || account.password_command.is_some();
        let status = if has_auth { "configured" } else { "no auth" };
        println!(
            "  Matrix #{}: {} @ {} ({})",
            i + 1,
            account.label,
            account.homeserver,
            status
        );
    }

    for (i, account) in config.accounts.slack.iter().enumerate() {
        let has_token = account.token.is_some() || account.token_command.is_some();
        let has_app_token = account.app_token.is_some() || account.app_token_command.is_some();
        let mode = if has_app_token {
            "socket mode"
        } else {
            "polling"
        };
        let status = if has_token { "configured" } else { "no token" };
        println!(
            "  Slack #{}: {} ({}, {})",
            i + 1,
            account.workspace,
            status,
            mode
        );
    }

    if config.accounts.discord.is_empty()
        && config.accounts.matrix.is_empty()
        && config.accounts.slack.is_empty()
    {
        println!("  (no accounts configured)");
        println!();
        println!("Add accounts to ~/.config/fumi/fumi.yaml");
    }
}

/// Send a message from the CLI.
fn send_message(
    config: &FumiConfig,
    protocol: &str,
    channel: &str,
    message: &str,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        match protocol {
            "discord" => {
                let account = config
                    .accounts
                    .discord
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("no Discord account configured"))?;
                let token = config::resolve_token(&account.token, &account.token_command)?
                    .ok_or_else(|| anyhow::anyhow!("no Discord token"))?;
                let mut backend = discord::DiscordBackend::new(&token);
                backend.connect().await.map_err(|e| anyhow::anyhow!("{e}"))?;
                backend
                    .send_message(channel, message)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                println!("Message sent to Discord {channel}");
            }
            "matrix" => {
                let account = config
                    .accounts
                    .matrix
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("no Matrix account configured"))?;
                let token = config::resolve_token(&account.token, &account.token_command)?;
                let mut backend =
                    matrix::MatrixBackend::new(&account.homeserver, &account.username, token.as_deref());
                backend.connect().await.map_err(|e| anyhow::anyhow!("{e}"))?;
                backend
                    .send_message(channel, message)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                println!("Message sent to Matrix {channel}");
            }
            "slack" => {
                let account = config
                    .accounts
                    .slack
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("no Slack account configured"))?;
                let token = config::resolve_token(&account.token, &account.token_command)?
                    .ok_or_else(|| anyhow::anyhow!("no Slack token"))?;
                let mut backend = slack::SlackBackend::new(&account.workspace, &token);
                backend.connect().await.map_err(|e| anyhow::anyhow!("{e}"))?;
                backend
                    .send_message(channel, message)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                println!("Message sent to Slack {channel}");
            }
            other => {
                anyhow::bail!("unknown protocol: {other} (use: discord, matrix, slack)");
            }
        }
        Ok(())
    })
}

/// Check daemon health.
fn check_health() {
    let daemon = daemon::FumiDaemon::new(FumiConfig::default());
    if daemon.is_running() {
        println!("fumi daemon: running");
        println!("  socket: {}", daemon::FumiDaemon::socket_path().display());
    } else {
        println!("fumi daemon: not running");
    }
}

/// Connect all configured protocol backends and populate the unified store.
async fn connect_all_backends(config: &FumiConfig, ui_state: &mut ChatUiState) {
    // Discord
    for account in &config.accounts.discord {
        match config::resolve_token(&account.token, &account.token_command) {
            Ok(Some(token)) => {
                let mut backend = discord::DiscordBackend::new(&token);
                if let Err(e) = backend.connect().await {
                    tracing::error!(label = %account.label, "Discord connect failed: {e}");
                } else {
                    ui_state
                        .store
                        .merge_servers(Protocol::Discord, backend.servers());
                    tracing::info!(label = %account.label, "Discord connected");
                }
            }
            Ok(None) => {
                tracing::warn!(label = %account.label, "Discord: no token available");
            }
            Err(e) => {
                tracing::error!(label = %account.label, "Discord token resolution failed: {e}");
            }
        }
    }

    // Matrix
    for account in &config.accounts.matrix {
        let token = config::resolve_token(&account.token, &account.token_command)
            .ok()
            .flatten();
        let mut backend =
            matrix::MatrixBackend::new(&account.homeserver, &account.username, token.as_deref());
        if let Err(e) = backend.connect().await {
            tracing::error!(label = %account.label, "Matrix connect failed: {e}");
        } else {
            ui_state
                .store
                .merge_servers(Protocol::Matrix, backend.servers());
            tracing::info!(label = %account.label, "Matrix connected");
        }
    }

    // Slack
    for account in &config.accounts.slack {
        match config::resolve_token(&account.token, &account.token_command) {
            Ok(Some(token)) => {
                let app_token = config::resolve_token(&account.app_token, &account.app_token_command)
                    .ok()
                    .flatten();
                let mut backend = slack::SlackBackend::with_app_token(
                    &account.workspace,
                    &token,
                    app_token.as_deref(),
                );
                if let Err(e) = backend.connect().await {
                    tracing::error!(label = %account.label, "Slack connect failed: {e}");
                } else {
                    ui_state
                        .store
                        .merge_servers(Protocol::Slack, backend.servers());
                    tracing::info!(label = %account.label, "Slack connected");
                }
            }
            Ok(None) => {
                tracing::warn!(label = %account.label, "Slack: no token available");
            }
            Err(e) => {
                tracing::error!(label = %account.label, "Slack token resolution failed: {e}");
            }
        }
    }

    // Auto-select first server if available.
    let first_ids = ui_state.store.servers().first().map(|s| {
        let server_id = s.id.clone();
        let channel_id = s.channels.first().map(|c| c.id.clone());
        (server_id, channel_id)
    });
    if let Some((server_id, channel_id)) = first_ids {
        ui_state.store.set_active_server(&server_id);
        if let Some(ch_id) = channel_id {
            ui_state.store.set_active_channel(&ch_id);
        }
    }
}
