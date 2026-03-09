//! Configuration module via shikumi.
//!
//! Multi-account configuration for Discord, Matrix, and Slack.
//! Config file: `~/.config/fumi/fumi.yaml`
//! Env override: `$FUMI_CONFIG`
//! Env prefix: `FUMI_`

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level fumi configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FumiConfig {
    #[serde(default)]
    pub accounts: AccountsConfig,
    #[serde(default)]
    pub behavior: BehaviorConfig,
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub voice: VoiceConfig,
    #[serde(default)]
    pub keybindings: KeybindingsConfig,
}

/// Multi-account configuration across protocols.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountsConfig {
    /// Discord accounts (bot or user token).
    #[serde(default)]
    pub discord: Vec<DiscordAccount>,
    /// Matrix accounts.
    #[serde(default)]
    pub matrix: Vec<MatrixAccount>,
    /// Slack workspace accounts.
    #[serde(default)]
    pub slack: Vec<SlackAccount>,
}

/// A Discord account configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordAccount {
    /// Account label (for display in UI).
    #[serde(default = "default_label")]
    pub label: String,
    /// Discord bot or user token. Prefer `token_command`.
    pub token: Option<String>,
    /// Shell command that outputs the token on stdout.
    pub token_command: Option<String>,
}

/// A Matrix account configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixAccount {
    /// Account label (for display in UI).
    #[serde(default = "default_label")]
    pub label: String,
    /// Homeserver URL (e.g. "https://matrix.org").
    pub homeserver: String,
    /// Matrix user ID (e.g. "@user:matrix.org").
    pub username: String,
    /// Access token (preferred).
    pub token: Option<String>,
    /// Shell command that outputs the token on stdout.
    pub token_command: Option<String>,
    /// Shell command that outputs the password on stdout.
    pub password_command: Option<String>,
}

/// A Slack workspace account configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackAccount {
    /// Account label (for display in UI).
    #[serde(default = "default_label")]
    pub label: String,
    /// Workspace name (for display).
    pub workspace: String,
    /// Bot token (xoxb-...) or user token (xoxp-...).
    pub token: Option<String>,
    /// Shell command that outputs the token on stdout.
    pub token_command: Option<String>,
    /// App-level token (xapp-...) for Socket Mode.
    pub app_token: Option<String>,
    /// Shell command that outputs the app token on stdout.
    pub app_token_command: Option<String>,
}

/// Behavior configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfig {
    /// Enable desktop notifications.
    #[serde(default = "default_true")]
    pub notifications: bool,
    /// Notification filter: all, mentions, none.
    #[serde(default = "default_notification_filter")]
    pub notification_filter: String,
    /// Start in daemon mode (background connections).
    #[serde(default)]
    pub daemon_mode: bool,
    /// Channels to open on startup (e.g. "personal-discord/#general").
    #[serde(default)]
    pub startup_channels: Vec<String>,
    /// Enable sound for notifications.
    #[serde(default = "default_true")]
    pub sound: bool,
    /// Do-not-disturb hours (start_hour, end_hour).
    #[serde(default)]
    pub dnd_hours: Option<(u8, u8)>,
}

/// Appearance configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceConfig {
    /// Theme name (e.g. "nord").
    #[serde(default = "default_theme_name")]
    pub theme: String,
    /// Background color hex.
    #[serde(default = "default_bg")]
    pub background: String,
    /// Foreground color hex.
    #[serde(default = "default_fg")]
    pub foreground: String,
    /// Accent color hex.
    #[serde(default = "default_accent")]
    pub accent: String,
    /// Compact message display mode.
    #[serde(default = "default_true")]
    pub compact_mode: bool,
    /// Show user avatars.
    #[serde(default = "default_true")]
    pub show_avatars: bool,
    /// Font size in points.
    #[serde(default = "default_font_size")]
    pub font_size: f32,
}

/// Voice configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceConfig {
    /// Echo cancellation.
    #[serde(default = "default_true")]
    pub echo_cancellation: bool,
    /// Noise suppression.
    #[serde(default = "default_true")]
    pub noise_suppression: bool,
    /// Input volume (0.0 to 2.0).
    #[serde(default = "default_input_volume")]
    pub input_volume: f32,
    /// Push-to-talk mode.
    #[serde(default)]
    pub push_to_talk: bool,
    /// Push-to-talk key.
    #[serde(default = "default_ptt_key")]
    pub push_to_talk_key: String,
}

/// Custom keybinding overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeybindingsConfig {
    /// Override keybindings (action -> key combo string).
    #[serde(default)]
    pub overrides: std::collections::HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

impl Default for FumiConfig {
    fn default() -> Self {
        Self {
            accounts: AccountsConfig::default(),
            behavior: BehaviorConfig::default(),
            appearance: AppearanceConfig::default(),
            voice: VoiceConfig::default(),
            keybindings: KeybindingsConfig::default(),
        }
    }
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            notifications: true,
            notification_filter: default_notification_filter(),
            daemon_mode: false,
            startup_channels: Vec::new(),
            sound: true,
            dnd_hours: None,
        }
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: default_theme_name(),
            background: default_bg(),
            foreground: default_fg(),
            accent: default_accent(),
            compact_mode: true,
            show_avatars: true,
            font_size: default_font_size(),
        }
    }
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            echo_cancellation: true,
            noise_suppression: true,
            input_volume: default_input_volume(),
            push_to_talk: false,
            push_to_talk_key: default_ptt_key(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_input_volume() -> f32 {
    1.0
}
fn default_bg() -> String {
    "#2e3440".into()
}
fn default_fg() -> String {
    "#eceff4".into()
}
fn default_accent() -> String {
    "#88c0d0".into()
}
fn default_label() -> String {
    "default".into()
}
fn default_theme_name() -> String {
    "nord".into()
}
fn default_notification_filter() -> String {
    "mentions".into()
}
fn default_font_size() -> f32 {
    14.0
}
fn default_ptt_key() -> String {
    "Space".into()
}

// ---------------------------------------------------------------------------
// Token resolution
// ---------------------------------------------------------------------------

/// Resolve a token: use the direct value, or run the command to get it.
pub fn resolve_token(
    token: &Option<String>,
    command: &Option<String>,
) -> anyhow::Result<Option<String>> {
    if let Some(t) = token {
        return Ok(Some(t.clone()));
    }
    if let Some(cmd) = command {
        let output = std::process::Command::new("sh")
            .args(["-c", cmd])
            .output()?;
        if output.status.success() {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if !token.is_empty() {
                return Ok(Some(token));
            }
        }
        anyhow::bail!("token command failed: {cmd}");
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Load configuration from the specified path or discover via shikumi.
pub fn load(override_path: &Option<PathBuf>) -> anyhow::Result<FumiConfig> {
    let path = match override_path {
        Some(p) => p.clone(),
        None => match shikumi::ConfigDiscovery::new("fumi")
            .env_override("FUMI_CONFIG")
            .discover()
        {
            Ok(p) => p,
            Err(_) => {
                tracing::info!("no config file found, using defaults");
                return Ok(FumiConfig::default());
            }
        },
    };

    let store = shikumi::ConfigStore::<FumiConfig>::load(&path, "FUMI_")?;
    Ok(FumiConfig::clone(&store.get()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_serde_roundtrip() {
        let config = FumiConfig::default();
        let yaml = serde_json::to_string(&config).expect("serialize");
        let back: FumiConfig = serde_json::from_str(&yaml).expect("deserialize");
        assert_eq!(back.appearance.background, "#2e3440");
        assert_eq!(back.appearance.foreground, "#eceff4");
        assert!(back.behavior.notifications);
    }

    #[test]
    fn accounts_config_empty_by_default() {
        let config = AccountsConfig::default();
        assert!(config.discord.is_empty());
        assert!(config.matrix.is_empty());
        assert!(config.slack.is_empty());
    }

    #[test]
    fn resolve_token_direct() {
        let result = resolve_token(&Some("tok123".into()), &None).unwrap();
        assert_eq!(result, Some("tok123".into()));
    }

    #[test]
    fn resolve_token_none() {
        let result = resolve_token(&None, &None).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_token_command() {
        let result = resolve_token(&None, &Some("echo test-token".into())).unwrap();
        assert_eq!(result, Some("test-token".into()));
    }

    #[test]
    fn voice_config_defaults() {
        let config = VoiceConfig::default();
        assert!(config.echo_cancellation);
        assert!(config.noise_suppression);
        assert!((config.input_volume - 1.0).abs() < f32::EPSILON);
        assert!(!config.push_to_talk);
    }

    #[test]
    fn appearance_config_defaults() {
        let config = AppearanceConfig::default();
        assert_eq!(config.theme, "nord");
        assert!((config.font_size - 14.0).abs() < f32::EPSILON);
    }
}
