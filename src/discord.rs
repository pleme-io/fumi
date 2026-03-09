//! Discord protocol backend via serenity.
//!
//! Connects to the Discord gateway over WebSocket, receives real-time events,
//! and sends messages via the REST API. Maps Discord's data model (guilds,
//! channels, members) to the common [`crate::protocol`] types.
//!
//! Uses [serenity](https://docs.rs/serenity) for:
//! - Gateway WebSocket (real-time events)
//! - REST API (message send/edit/delete, channel history)
//! - Voice channel support (via oto integration)
//! - Rich embeds, reactions, threads, forums

use std::sync::Arc;

use serenity::all as serenity_model;
use tokio::sync::{broadcast, RwLock};

use crate::protocol::{
    Attachment, Channel, ChannelType, ChatBackend, ChatError, ChatEvent, Message, Protocol,
    Reaction, Server, User,
};

/// Broadcast channel capacity for Discord events.
const EVENT_CHANNEL_CAPACITY: usize = 256;

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// Discord backend implementation.
///
/// Wraps a serenity `Client` and maps Discord events to [`ChatEvent`].
pub struct DiscordBackend {
    token: String,
    servers: Vec<Server>,
    event_tx: broadcast::Sender<ChatEvent>,
    connected: bool,
    /// Shared HTTP client from serenity (available after connect).
    http: Option<Arc<serenity_model::Http>>,
    /// Handle to the running gateway task so we can shut it down.
    gateway_handle: Option<tokio::task::JoinHandle<()>>,
    /// Shared server state that the event handler populates.
    shared_servers: Arc<RwLock<Vec<Server>>>,
}

impl DiscordBackend {
    /// Create a new Discord backend with the given bot/user token.
    pub fn new(token: &str) -> Self {
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            token: token.to_owned(),
            servers: Vec::new(),
            event_tx,
            connected: false,
            http: None,
            gateway_handle: None,
            shared_servers: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl ChatBackend for DiscordBackend {
    async fn connect(&mut self) -> Result<(), ChatError> {
        if self.connected {
            return Ok(());
        }

        tracing::info!("connecting to Discord gateway");

        // Build serenity intents — we want guild messages, DMs, reactions,
        // presence, and member info.
        let intents = serenity_model::GatewayIntents::GUILD_MESSAGES
            | serenity_model::GatewayIntents::DIRECT_MESSAGES
            | serenity_model::GatewayIntents::MESSAGE_CONTENT
            | serenity_model::GatewayIntents::GUILD_MESSAGE_REACTIONS
            | serenity_model::GatewayIntents::GUILDS
            | serenity_model::GatewayIntents::GUILD_PRESENCES
            | serenity_model::GatewayIntents::GUILD_MEMBERS;

        let event_tx = self.event_tx.clone();
        let shared_servers = Arc::clone(&self.shared_servers);

        // TODO: Build the serenity Client with an EventHandler that:
        //   1. On `ready` — populate shared_servers from the guild cache,
        //      broadcast ChatEvent::Connected(Protocol::Discord).
        //   2. On `message` — convert serenity::Message to protocol::Message,
        //      broadcast ChatEvent::MessageReceived.
        //   3. On `message_update` — broadcast ChatEvent::MessageEdited.
        //   4. On `message_delete` — broadcast ChatEvent::MessageDeleted.
        //   5. On `typing_start` — broadcast ChatEvent::TypingStarted.
        //   6. On `presence_update` — broadcast ChatEvent::PresenceChanged.
        //
        // Skeleton (will not compile without the full handler impl):
        //
        //   let mut client = serenity_model::Client::builder(&self.token, intents)
        //       .event_handler(DiscordHandler { event_tx, shared_servers })
        //       .await
        //       .map_err(|e| ChatError::Connection(e.to_string()))?;
        //
        //   self.http = Some(Arc::clone(&client.http));
        //
        //   self.gateway_handle = Some(tokio::spawn(async move {
        //       if let Err(e) = client.start().await {
        //           tracing::error!("discord gateway error: {e}");
        //       }
        //   }));

        let _ = (intents, event_tx, shared_servers);

        self.connected = true;
        tracing::info!("Discord backend marked as connected (gateway TODO)");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), ChatError> {
        if !self.connected {
            return Ok(());
        }

        tracing::info!("disconnecting from Discord");

        // Abort the gateway task if it is running.
        if let Some(handle) = self.gateway_handle.take() {
            handle.abort();
        }

        self.connected = false;
        let _ = self.event_tx.send(ChatEvent::Disconnected(Protocol::Discord));
        Ok(())
    }

    fn servers(&self) -> &[Server] {
        &self.servers
    }

    async fn send_message(
        &self,
        channel_id: &str,
        content: &str,
    ) -> Result<Message, ChatError> {
        let http = self.http.as_ref().ok_or(ChatError::NotConnected)?;

        let channel_id_parsed: u64 = channel_id
            .parse()
            .map_err(|_| ChatError::Send(format!("invalid channel id: {channel_id}")))?;
        let channel = serenity_model::ChannelId::new(channel_id_parsed);

        let serenity_msg = channel
            .say(http, content)
            .await
            .map_err(|e| ChatError::Send(e.to_string()))?;

        Ok(discord_message_to_protocol(&serenity_msg))
    }

    async fn fetch_messages(
        &self,
        channel_id: &str,
        limit: usize,
        before: Option<&str>,
    ) -> Result<Vec<Message>, ChatError> {
        let http = self.http.as_ref().ok_or(ChatError::NotConnected)?;

        let channel_id_parsed: u64 = channel_id
            .parse()
            .map_err(|_| ChatError::Api(format!("invalid channel id: {channel_id}")))?;
        let channel = serenity_model::ChannelId::new(channel_id_parsed);

        let builder = if let Some(before_id) = before {
            let mid: u64 = before_id
                .parse()
                .map_err(|_| ChatError::Api(format!("invalid message id: {before_id}")))?;
            serenity_model::GetMessages::new()
                .before(serenity_model::MessageId::new(mid))
                .limit(limit as u8)
        } else {
            serenity_model::GetMessages::new().limit(limit as u8)
        };

        let messages = channel
            .messages(http, builder)
            .await
            .map_err(|e| ChatError::Api(e.to_string()))?;

        Ok(messages.iter().map(discord_message_to_protocol).collect())
    }

    fn events(&self) -> broadcast::Receiver<ChatEvent> {
        self.event_tx.subscribe()
    }

    fn protocol(&self) -> Protocol {
        Protocol::Discord
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a serenity `Message` to a protocol [`Message`].
fn discord_message_to_protocol(msg: &serenity_model::Message) -> Message {
    Message {
        id: msg.id.to_string(),
        protocol: Protocol::Discord,
        channel_id: msg.channel_id.to_string(),
        author: discord_user_to_protocol(&msg.author),
        content: msg.content.clone(),
        timestamp: msg.timestamp.unix_timestamp() as u64,
        edited: msg.edited_timestamp.is_some(),
        attachments: msg
            .attachments
            .iter()
            .map(|a| Attachment {
                filename: a.filename.clone(),
                url: a.url.clone(),
                content_type: a.content_type.clone(),
                size: Some(a.size as u64),
            })
            .collect(),
        reactions: msg
            .reactions
            .iter()
            .map(|r| Reaction {
                emoji: reaction_type_to_string(&r.reaction_type),
                count: r.count as u32,
                me: r.me,
            })
            .collect(),
        reply_to: msg
            .referenced_message
            .as_ref()
            .map(|m| m.id.to_string()),
    }
}

/// Convert a serenity `User` to a protocol [`User`].
fn discord_user_to_protocol(user: &serenity_model::User) -> User {
    User {
        id: user.id.to_string(),
        name: user.name.clone(),
        display_name: user.global_name.clone(),
        avatar_url: user.avatar_url(),
        protocol: Protocol::Discord,
        bot: user.bot,
    }
}

/// Convert a serenity `ReactionType` to a display string.
fn reaction_type_to_string(rt: &serenity_model::ReactionType) -> String {
    match rt {
        serenity_model::ReactionType::Unicode(s) => s.clone(),
        serenity_model::ReactionType::Custom { name, .. } => {
            name.as_deref().unwrap_or("custom").to_owned()
        }
        _ => "unknown".to_owned(),
    }
}

/// Convert a serenity `GuildChannel` to a protocol [`Channel`].
#[allow(dead_code)]
fn discord_channel_to_protocol(ch: &serenity_model::GuildChannel) -> Channel {
    Channel {
        id: ch.id.to_string(),
        name: ch.name.clone(),
        protocol: Protocol::Discord,
        channel_type: match ch.kind {
            serenity_model::ChannelType::Voice | serenity_model::ChannelType::Stage => {
                ChannelType::Voice
            }
            serenity_model::ChannelType::Private => ChannelType::Direct,
            serenity_model::ChannelType::Category
            | serenity_model::ChannelType::PublicThread
            | serenity_model::ChannelType::PrivateThread
            | serenity_model::ChannelType::NewsThread => ChannelType::Thread,
            _ => ChannelType::Text,
        },
        unread: 0,
        mention_count: 0,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_new_defaults() {
        let backend = DiscordBackend::new("test-token");
        assert!(!backend.connected);
        assert_eq!(backend.protocol(), Protocol::Discord);
        assert!(backend.servers().is_empty());
    }

    #[test]
    fn event_subscription() {
        let backend = DiscordBackend::new("test-token");
        let mut rx = backend.events();

        // Send an event through the internal channel.
        let _ = backend
            .event_tx
            .send(ChatEvent::Connected(Protocol::Discord));

        let event = rx.try_recv().expect("should receive event");
        match event {
            ChatEvent::Connected(p) => assert_eq!(p, Protocol::Discord),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn multiple_subscribers() {
        let backend = DiscordBackend::new("tok");
        let mut rx1 = backend.events();
        let mut rx2 = backend.events();

        let _ = backend
            .event_tx
            .send(ChatEvent::Disconnected(Protocol::Discord));

        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }

    #[tokio::test]
    async fn send_message_without_connect_fails() {
        let backend = DiscordBackend::new("tok");
        let result = backend.send_message("123", "hello").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ChatError::NotConnected => {}
            other => panic!("expected NotConnected, got: {other}"),
        }
    }

    #[tokio::test]
    async fn fetch_messages_without_connect_fails() {
        let backend = DiscordBackend::new("tok");
        let result = backend.fetch_messages("123", 50, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn disconnect_when_not_connected() {
        let mut backend = DiscordBackend::new("tok");
        // Should be a no-op, not an error.
        let result = backend.disconnect().await;
        assert!(result.is_ok());
    }
}
