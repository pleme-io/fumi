//! Rhai scripting integration for Fumi.
//!
//! Loads user scripts from `~/.config/fumi/scripts/*.rhai` and exposes
//! app-specific functions for chat operations.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use soushi::ScriptEngine;

/// Script hook events that can trigger user scripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptEvent {
    /// A new message was received.
    MessageReceived { channel: String, author: String },
    /// Connected to a protocol backend.
    Connected { protocol: String },
    /// Disconnected from a protocol backend.
    Disconnected { protocol: String },
    /// Channel was switched.
    ChannelSwitched { channel: String },
}

/// Manages the Rhai scripting engine with fumi-specific functions.
pub struct FumiScripting {
    engine: ScriptEngine,
    /// Compiled event hook scripts (ASTs keyed by event name).
    hooks: std::collections::HashMap<String, soushi::rhai::AST>,
}

impl FumiScripting {
    /// Create a new scripting engine with fumi chat functions registered.
    ///
    /// Registers: `fumi.send(msg)`, `fumi.switch_channel(name)`,
    /// `fumi.list_channels()`, `fumi.get_messages(count)`.
    ///
    /// The `action_tx` channel is used to send actions back to the main event loop.
    #[must_use]
    pub fn new(action_tx: Arc<Mutex<Vec<ScriptAction>>>) -> Self {
        let mut engine = ScriptEngine::new();
        engine.register_builtin_log();
        engine.register_builtin_env();
        engine.register_builtin_string();

        // fumi.send(msg)
        let tx = action_tx.clone();
        engine.register_fn("fumi_send", move |msg: &str| {
            if let Ok(mut actions) = tx.lock() {
                actions.push(ScriptAction::Send(msg.to_string()));
            }
        });

        // fumi.switch_channel(name)
        let tx = action_tx.clone();
        engine.register_fn("fumi_switch_channel", move |name: &str| {
            if let Ok(mut actions) = tx.lock() {
                actions.push(ScriptAction::SwitchChannel(name.to_string()));
            }
        });

        // fumi.list_channels() -> returns empty string (async data not available in script context)
        let tx = action_tx.clone();
        engine.register_fn("fumi_list_channels", move || -> String {
            if let Ok(mut actions) = tx.lock() {
                actions.push(ScriptAction::ListChannels);
            }
            String::new()
        });

        // fumi.get_messages(count)
        let tx = action_tx;
        engine.register_fn("fumi_get_messages", move |count: i64| -> String {
            if let Ok(mut actions) = tx.lock() {
                actions.push(ScriptAction::GetMessages(count as usize));
            }
            String::new()
        });

        Self {
            engine,
            hooks: std::collections::HashMap::new(),
        }
    }

    /// Load all scripts from the scripts directory.
    ///
    /// Looks in `~/.config/fumi/scripts/` by default.
    pub fn load_scripts(&mut self) -> Result<Vec<String>, soushi::SoushiError> {
        let scripts_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fumi")
            .join("scripts");

        if !scripts_dir.is_dir() {
            tracing::debug!(path = %scripts_dir.display(), "scripts directory not found, skipping");
            return Ok(Vec::new());
        }

        self.engine.load_scripts_dir(&scripts_dir)
    }

    /// Register an event hook script.
    pub fn register_hook(&mut self, event_name: &str, script: &str) -> Result<(), soushi::SoushiError> {
        let ast = self.engine.compile(script)?;
        self.hooks.insert(event_name.to_string(), ast);
        Ok(())
    }

    /// Fire an event, running any registered hook scripts.
    pub fn fire_event(&self, event: &ScriptEvent) {
        let event_name = match event {
            ScriptEvent::MessageReceived { .. } => "message_received",
            ScriptEvent::Connected { .. } => "connected",
            ScriptEvent::Disconnected { .. } => "disconnected",
            ScriptEvent::ChannelSwitched { .. } => "channel_switched",
        };

        if let Some(ast) = self.hooks.get(event_name) {
            if let Err(e) = self.engine.eval_ast(ast) {
                tracing::error!(event = event_name, error = %e, "script hook failed");
            }
        }
    }

    /// Evaluate an ad-hoc script string.
    pub fn eval(&self, script: &str) -> Result<soushi::rhai::Dynamic, soushi::SoushiError> {
        self.engine.eval(script)
    }
}

/// Actions that scripts can request from the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptAction {
    /// Send a message to the active channel.
    Send(String),
    /// Switch to a named channel.
    SwitchChannel(String),
    /// Request the channel list.
    ListChannels,
    /// Request recent messages.
    GetMessages(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_engine() -> (FumiScripting, Arc<Mutex<Vec<ScriptAction>>>) {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let engine = FumiScripting::new(actions.clone());
        (engine, actions)
    }

    #[test]
    fn send_function_queues_action() {
        let (engine, actions) = make_engine();
        engine.eval(r#"fumi_send("hello world")"#).unwrap();
        let actions = actions.lock().unwrap();
        assert_eq!(actions[0], ScriptAction::Send("hello world".to_string()));
    }

    #[test]
    fn switch_channel_function_queues_action() {
        let (engine, actions) = make_engine();
        engine.eval(r#"fumi_switch_channel("general")"#).unwrap();
        let actions = actions.lock().unwrap();
        assert_eq!(actions[0], ScriptAction::SwitchChannel("general".to_string()));
    }

    #[test]
    fn list_channels_function_queues_action() {
        let (engine, actions) = make_engine();
        engine.eval("fumi_list_channels()").unwrap();
        let actions = actions.lock().unwrap();
        assert_eq!(actions[0], ScriptAction::ListChannels);
    }

    #[test]
    fn get_messages_function_queues_action() {
        let (engine, actions) = make_engine();
        engine.eval("fumi_get_messages(10)").unwrap();
        let actions = actions.lock().unwrap();
        assert_eq!(actions[0], ScriptAction::GetMessages(10));
    }

    #[test]
    fn fire_event_with_no_hook_is_noop() {
        let (engine, _actions) = make_engine();
        engine.fire_event(&ScriptEvent::ChannelSwitched {
            channel: "general".to_string(),
        });
    }

    #[test]
    fn register_and_fire_hook() {
        let (mut engine, actions) = make_engine();
        engine
            .register_hook("connected", r#"fumi_send("bot online")"#)
            .unwrap();
        engine.fire_event(&ScriptEvent::Connected {
            protocol: "discord".to_string(),
        });
        let actions = actions.lock().unwrap();
        assert_eq!(actions[0], ScriptAction::Send("bot online".to_string()));
    }

    #[test]
    fn load_scripts_missing_dir_returns_empty() {
        let (mut engine, _actions) = make_engine();
        let result = engine.load_scripts();
        assert!(result.is_ok());
    }

    #[test]
    fn eval_arbitrary_script() {
        let (engine, _actions) = make_engine();
        let result = engine.eval("40 + 2").unwrap();
        assert_eq!(result.as_int().unwrap(), 42);
    }
}
