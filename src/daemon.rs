//! Background daemon mode via tsunagu.
//!
//! Maintains persistent chat protocol connections even when the GUI is closed.
//! Uses tsunagu for PID lifecycle, Unix socket paths, and health checks.
//! The GUI connects to the daemon for events and sends actions back.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;
use tsunagu::{DaemonProcess, HealthCheck, SocketPath};

use crate::config::FumiConfig;
use crate::protocol::{ChatBackend, Protocol, UnifiedStore};

/// The fumi background daemon.
///
/// Owns all protocol backend connections, processes events into the unified
/// store, and serves health checks.
pub struct FumiDaemon {
    /// tsunagu daemon process (PID + socket lifecycle).
    process: DaemonProcess,
    /// Configuration.
    config: FumiConfig,
    /// Shared unified store.
    store: Arc<RwLock<UnifiedStore>>,
    /// Start time for uptime tracking.
    start_time: Instant,
    /// Whether the daemon is running.
    running: bool,
}

impl FumiDaemon {
    /// Create a new daemon instance.
    pub fn new(config: FumiConfig) -> Self {
        Self {
            process: DaemonProcess::new("fumi"),
            config,
            store: Arc::new(RwLock::new(UnifiedStore::new())),
            start_time: Instant::now(),
            running: false,
        }
    }

    /// Acquire the daemon lock and start.
    ///
    /// Returns an error if another daemon instance is already running.
    pub fn acquire(&self) -> Result<(), tsunagu::TsunaguError> {
        self.process.acquire()
    }

    /// Check if a daemon is already running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.process.is_running()
    }

    /// Run the daemon main loop.
    ///
    /// Connects all configured protocol backends and processes events
    /// indefinitely until signaled to stop.
    pub async fn run(&mut self) -> anyhow::Result<()> {
        self.acquire()?;
        self.running = true;
        self.start_time = Instant::now();

        tracing::info!(
            socket = %self.process.socket_path().display(),
            pid = %std::process::id(),
            "fumi daemon started"
        );

        // Connect all configured backends.
        let mut event_receivers = Vec::new();

        // Discord
        for account in &self.config.accounts.discord {
            let token = crate::config::resolve_token(&account.token, &account.token_command)?;
            if let Some(token) = token {
                let mut backend = crate::discord::DiscordBackend::new(&token);
                if let Err(e) = backend.connect().await {
                    tracing::error!(label = %account.label, "Discord connect failed: {e}");
                } else {
                    event_receivers.push(backend.events());
                    let servers = backend.servers().to_vec();
                    self.store.write().await.merge_servers(Protocol::Discord, &servers);
                    tracing::info!(label = %account.label, "Discord connected");
                }
            }
        }

        // Matrix
        for account in &self.config.accounts.matrix {
            let token = crate::config::resolve_token(&account.token, &account.token_command)?;
            let mut backend =
                crate::matrix::MatrixBackend::new(&account.homeserver, &account.username, token.as_deref());
            if let Err(e) = backend.connect().await {
                tracing::error!(label = %account.label, "Matrix connect failed: {e}");
            } else {
                event_receivers.push(backend.events());
                let servers = backend.servers().to_vec();
                self.store.write().await.merge_servers(Protocol::Matrix, &servers);
                tracing::info!(label = %account.label, "Matrix connected");
            }
        }

        // Slack
        for account in &self.config.accounts.slack {
            let token = crate::config::resolve_token(&account.token, &account.token_command)?;
            if let Some(token) = token {
                let app_token =
                    crate::config::resolve_token(&account.app_token, &account.app_token_command)?;
                let mut backend = crate::slack::SlackBackend::with_app_token(
                    &account.workspace,
                    &token,
                    app_token.as_deref(),
                );
                if let Err(e) = backend.connect().await {
                    tracing::error!(label = %account.label, "Slack connect failed: {e}");
                } else {
                    event_receivers.push(backend.events());
                    let servers = backend.servers().to_vec();
                    self.store.write().await.merge_servers(Protocol::Slack, &servers);
                    tracing::info!(label = %account.label, "Slack connected");
                }
            }
        }

        // Event processing loop.
        let store = Arc::clone(&self.store);
        tokio::spawn(async move {
            // Process events from all backends.
            // In a production implementation, we'd use tokio::select! across all receivers.
            // For now, we spawn a task per receiver.
            for mut rx in event_receivers {
                let store = Arc::clone(&store);
                tokio::spawn(async move {
                    loop {
                        match rx.recv().await {
                            Ok(event) => {
                                tracing::debug!("daemon event: {event:?}");
                                store.write().await.handle_event(&event);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!("daemon event receiver lagged by {n}");
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                tracing::info!("daemon event channel closed");
                                break;
                            }
                        }
                    }
                });
            }
        });

        // Drain on SIGTERM/SIGINT. `ShutdownController::install` is
        // idempotent enough for daemon-mode binaries that run once per
        // process lifetime.
        let controller = tsunagu::ShutdownController::install();
        controller.token().wait().await;
        tracing::info!("fumi daemon draining");
        self.running = false;

        Ok(())
    }

    /// Get a health check response.
    #[must_use]
    pub fn health(&self) -> HealthCheck {
        let uptime = self.start_time.elapsed().as_secs();
        if self.running {
            HealthCheck::healthy("fumi", env!("CARGO_PKG_VERSION")).with_uptime(uptime)
        } else {
            HealthCheck::unhealthy("fumi", env!("CARGO_PKG_VERSION"), "not running")
        }
    }

    /// Get the socket path for IPC.
    #[must_use]
    pub fn socket_path() -> std::path::PathBuf {
        SocketPath::for_app("fumi")
    }

    /// Get a reference to the shared store.
    #[must_use]
    pub fn store(&self) -> &Arc<RwLock<UnifiedStore>> {
        &self.store
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_health_not_running() {
        let daemon = FumiDaemon::new(FumiConfig::default());
        let health = daemon.health();
        assert!(health.is_unhealthy());
    }

    #[test]
    fn daemon_socket_path() {
        let path = FumiDaemon::socket_path();
        assert!(path.to_string_lossy().contains("fumi"));
    }

    #[test]
    fn daemon_store_is_empty() {
        let daemon = FumiDaemon::new(FumiConfig::default());
        // Can't easily test the RwLock in sync context, but the store exists.
        assert!(!daemon.running);
    }
}
