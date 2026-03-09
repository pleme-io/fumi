use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FumiConfig {
    #[serde(default)]
    pub accounts: AccountsConfig,
    #[serde(default)]
    pub notifications: NotificationConfig,
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub voice: VoiceConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountsConfig {
    pub discord: Option<DiscordAccount>,
    pub matrix: Option<MatrixAccount>,
    pub slack: Vec<SlackAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordAccount {
    /// Discord bot or user token
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixAccount {
    pub homeserver: String,
    pub username: String,
    /// Token or password (token preferred)
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackAccount {
    pub workspace: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub sound: bool,
    #[serde(default)]
    pub dnd_hours: Option<(u8, u8)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceConfig {
    #[serde(default = "default_true")]
    pub echo_cancellation: bool,
    #[serde(default = "default_true")]
    pub noise_suppression: bool,
    #[serde(default = "default_input_volume")]
    pub input_volume: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceConfig {
    #[serde(default = "default_bg")]
    pub background: String,
    #[serde(default = "default_fg")]
    pub foreground: String,
    #[serde(default = "default_accent")]
    pub accent: String,
    #[serde(default = "default_true")]
    pub compact_mode: bool,
    #[serde(default = "default_true")]
    pub show_avatars: bool,
}

impl Default for FumiConfig {
    fn default() -> Self {
        Self {
            accounts: AccountsConfig::default(),
            notifications: NotificationConfig::default(),
            appearance: AppearanceConfig::default(),
            voice: VoiceConfig::default(),
        }
    }
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self { enabled: true, sound: true, dnd_hours: None }
    }
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self { echo_cancellation: true, noise_suppression: true, input_volume: default_input_volume() }
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            background: default_bg(),
            foreground: default_fg(),
            accent: default_accent(),
            compact_mode: true,
            show_avatars: true,
        }
    }
}

fn default_true() -> bool { true }
fn default_input_volume() -> f32 { 1.0 }
fn default_bg() -> String { "#2e3440".into() }
fn default_fg() -> String { "#eceff4".into() }
fn default_accent() -> String { "#88c0d0".into() }

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
