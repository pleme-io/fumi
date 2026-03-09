//! Fumi (文) — GPU-rendered multi-protocol chat client.
//!
//! Unified chat interface for Discord, Matrix, and Slack:
//! - GPU-accelerated UI via garasu + egaku widgets
//! - Rich text (markdown, embeds, reactions) via fude
//! - Voice chat via oto
//! - Multi-protocol: one interface, many backends
//! - Hot-reloadable configuration via shikumi

mod config;
mod discord;
mod matrix;
mod protocol;
mod render;
mod slack;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

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
    /// Launch the GUI
    Open,
    /// Start the background daemon (maintains connections)
    Daemon,
    /// List configured accounts
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
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let config = config::load(&cli.config)?;

    match cli.command {
        None | Some(Commands::Open) => {
            tracing::info!("launching fumi");
            // TODO: Initialize garasu GPU context
            // TODO: Create winit window with chat UI (egaku widgets)
            // TODO: Connect to configured chat protocols
        }
        Some(Commands::Daemon) => {
            tracing::info!("starting fumi daemon");
            // TODO: Maintain persistent connections via tsunagu daemon
        }
        Some(Commands::Accounts) => {
            // TODO: List configured accounts across protocols
        }
        Some(Commands::Send { protocol, channel, message }) => {
            tracing::info!("sending to {protocol}/{channel}: {message}");
            // TODO: Send via appropriate protocol backend
        }
    }

    Ok(())
}
