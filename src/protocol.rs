//! Protocol abstraction layer.
//!
//! Defines the common [`ChatBackend`] trait and shared types that Discord,
//! Matrix, and Slack backends implement. Every protocol maps its native data
//! model to these types so the UI and daemon layers can treat all protocols
//! uniformly.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Protocol identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Protocol {
    Discord,
    Matrix,
    Slack,
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Discord => write!(f, "discord"),
            Self::Matrix => write!(f, "matrix"),
            Self::Slack => write!(f, "slack"),
        }
    }
}

/// Channel type — maps to protocol-specific concepts (text channel, DM,
/// room, thread, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelType {
    Text,
    Voice,
    Direct,
    Group,
    Thread,
}

/// User presence status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresenceStatus {
    Online,
    Idle,
    DoNotDisturb,
    Offline,
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A chat message from any protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub protocol: Protocol,
    pub channel_id: String,
    pub author: User,
    pub content: String,
    pub timestamp: u64,
    pub edited: bool,
    pub attachments: Vec<Attachment>,
    pub reactions: Vec<Reaction>,
    pub reply_to: Option<String>,
}

/// A user from any protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub protocol: Protocol,
    pub bot: bool,
}

impl User {
    /// Returns the display name if set, otherwise the username.
    #[must_use]
    pub fn effective_name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }
}

/// A channel (text channel, DM, room, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub channel_type: ChannelType,
    pub server_id: Option<String>,
    pub topic: Option<String>,
    pub unread: u32,
    pub mention_count: u32,
}

/// A file attachment on a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub filename: String,
    pub url: String,
    pub content_type: Option<String>,
    pub size: Option<u64>,
}

/// A reaction (emoji + count) on a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reaction {
    pub emoji: String,
    pub count: u32,
    pub me: bool,
}

/// A server / guild / workspace — protocol-dependent grouping of channels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub icon_url: Option<String>,
    pub channels: Vec<Channel>,
}

/// Member of a channel with presence info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub user: User,
    pub presence: PresenceStatus,
    pub role: Option<String>,
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Events received asynchronously from chat protocols.
#[derive(Debug, Clone)]
pub enum ChatEvent {
    /// A new message was received.
    MessageReceived(Message),
    /// An existing message was edited.
    MessageEdited(Message),
    /// A message was deleted.
    MessageDeleted {
        channel_id: String,
        message_id: String,
    },
    /// A user started typing in a channel.
    TypingStarted {
        channel_id: String,
        user: User,
    },
    /// A user's presence changed.
    PresenceChanged {
        user_id: String,
        status: PresenceStatus,
    },
    /// Channel metadata was updated (name, topic, unread count, etc.).
    ChannelUpdated(Channel),
    /// Successfully connected to a protocol.
    Connected(Protocol),
    /// Disconnected from a protocol.
    Disconnected(Protocol),
    /// A protocol-level error occurred.
    Error {
        protocol: Protocol,
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur in chat protocol operations.
#[derive(thiserror::Error, Debug)]
pub enum ChatError {
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("send failed: {0}")]
    Send(String),
    #[error("API error: {0}")]
    Api(String),
    #[error("not connected")]
    NotConnected,
    #[error("channel not found: {0}")]
    ChannelNotFound(String),
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Trait that all protocol backends implement.
///
/// Each backend (Discord, Matrix, Slack) provides a concrete implementation
/// that maps its native API to these common operations.
pub trait ChatBackend: Send + Sync {
    /// Connect to the service (authenticate, open gateway/socket).
    fn connect(&mut self) -> impl std::future::Future<Output = Result<(), ChatError>> + Send;

    /// Disconnect from the service.
    fn disconnect(&mut self) -> impl std::future::Future<Output = Result<(), ChatError>> + Send;

    /// Get all servers/workspaces this backend is connected to.
    fn servers(&self) -> &[Server];

    /// Send a message to a channel. Returns the sent [`Message`].
    fn send_message(
        &self,
        channel_id: &str,
        content: &str,
    ) -> impl std::future::Future<Output = Result<Message, ChatError>> + Send;

    /// Fetch message history for a channel.
    ///
    /// `limit` controls how many messages to fetch; `before` is an optional
    /// message ID for pagination (fetch messages older than this ID).
    fn fetch_messages(
        &self,
        channel_id: &str,
        limit: usize,
        before: Option<&str>,
    ) -> impl std::future::Future<Output = Result<Vec<Message>, ChatError>> + Send;

    /// List members of a channel.
    fn list_members(
        &self,
        channel_id: &str,
    ) -> impl std::future::Future<Output = Result<Vec<Member>, ChatError>> + Send;

    /// Subscribe to real-time events. Returns a broadcast receiver that
    /// yields [`ChatEvent`] values.
    fn events(&self) -> tokio::sync::broadcast::Receiver<ChatEvent>;

    /// Protocol identifier for this backend.
    fn protocol(&self) -> Protocol;

    /// Whether the backend is currently connected.
    fn is_connected(&self) -> bool;
}

// ---------------------------------------------------------------------------
// Unified Store
// ---------------------------------------------------------------------------

/// Merged view of all protocol data, providing the data source for the UI.
///
/// Normalizes protocol-specific data into common types, maintains a merged
/// timeline across protocols, and tracks unread/mention counts.
pub struct UnifiedStore {
    /// All servers from all protocols.
    servers: Vec<Server>,
    /// Messages keyed by channel_id.
    messages: std::collections::HashMap<String, Vec<Message>>,
    /// Currently active channel.
    active_channel: Option<String>,
    /// Currently active server.
    active_server: Option<String>,
    /// Members of the active channel.
    members: Vec<Member>,
    /// Users currently typing in the active channel.
    typing: Vec<(String, User)>,
}

impl UnifiedStore {
    /// Create a new empty unified store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            servers: Vec::new(),
            messages: std::collections::HashMap::new(),
            active_channel: None,
            active_server: None,
            members: Vec::new(),
            typing: Vec::new(),
        }
    }

    /// Merge servers from a protocol backend into the store.
    pub fn merge_servers(&mut self, protocol: Protocol, new_servers: &[Server]) {
        // Remove old servers for this protocol.
        self.servers.retain(|s| s.protocol != protocol);
        // Add new ones.
        self.servers.extend_from_slice(new_servers);
    }

    /// All servers across all protocols.
    #[must_use]
    pub fn servers(&self) -> &[Server] {
        &self.servers
    }

    /// Get all channels from all servers, flattened.
    #[must_use]
    pub fn all_channels(&self) -> Vec<&Channel> {
        self.servers.iter().flat_map(|s| &s.channels).collect()
    }

    /// Get channels for the active server.
    #[must_use]
    pub fn active_server_channels(&self) -> Vec<&Channel> {
        if let Some(server_id) = &self.active_server {
            self.servers
                .iter()
                .find(|s| s.id == *server_id)
                .map_or_else(Vec::new, |s| s.channels.iter().collect())
        } else {
            Vec::new()
        }
    }

    /// Set the active server.
    pub fn set_active_server(&mut self, server_id: &str) {
        self.active_server = Some(server_id.to_owned());
    }

    /// Set the active channel.
    pub fn set_active_channel(&mut self, channel_id: &str) {
        self.active_channel = Some(channel_id.to_owned());
    }

    /// Get the active channel ID.
    #[must_use]
    pub fn active_channel(&self) -> Option<&str> {
        self.active_channel.as_deref()
    }

    /// Get the active server ID.
    #[must_use]
    pub fn active_server_id(&self) -> Option<&str> {
        self.active_server.as_deref()
    }

    /// Get messages for the active channel.
    #[must_use]
    pub fn active_messages(&self) -> &[Message] {
        self.active_channel
            .as_ref()
            .and_then(|ch| self.messages.get(ch))
            .map_or(&[], Vec::as_slice)
    }

    /// Get messages for a specific channel.
    #[must_use]
    pub fn channel_messages(&self, channel_id: &str) -> &[Message] {
        self.messages
            .get(channel_id)
            .map_or(&[], Vec::as_slice)
    }

    /// Store fetched messages for a channel.
    pub fn set_messages(&mut self, channel_id: &str, msgs: Vec<Message>) {
        self.messages.insert(channel_id.to_owned(), msgs);
    }

    /// Add a single message (from a real-time event).
    pub fn add_message(&mut self, msg: Message) {
        self.messages
            .entry(msg.channel_id.clone())
            .or_default()
            .push(msg);
    }

    /// Update a message (edit).
    pub fn update_message(&mut self, msg: Message) {
        if let Some(messages) = self.messages.get_mut(&msg.channel_id) {
            if let Some(existing) = messages.iter_mut().find(|m| m.id == msg.id) {
                *existing = msg;
            }
        }
    }

    /// Remove a message (delete).
    pub fn remove_message(&mut self, channel_id: &str, message_id: &str) {
        if let Some(messages) = self.messages.get_mut(channel_id) {
            messages.retain(|m| m.id != message_id);
        }
    }

    /// Set members for the active channel.
    pub fn set_members(&mut self, members: Vec<Member>) {
        self.members = members;
    }

    /// Get members of the active channel.
    #[must_use]
    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// Record that a user is typing.
    pub fn set_typing(&mut self, channel_id: &str, user: User) {
        if self.active_channel.as_deref() == Some(channel_id) {
            // Remove old entry for this user, then add new.
            self.typing.retain(|(_, u)| u.id != user.id);
            self.typing.push((channel_id.to_owned(), user));
        }
    }

    /// Clear typing indicator for a user (e.g. after they sent a message).
    pub fn clear_typing(&mut self, user_id: &str) {
        self.typing.retain(|(_, u)| u.id != user_id);
    }

    /// Get users currently typing in the active channel.
    #[must_use]
    pub fn typing_users(&self) -> Vec<&User> {
        self.typing.iter().map(|(_, u)| u).collect()
    }

    /// Process a chat event and update the store accordingly.
    pub fn handle_event(&mut self, event: &ChatEvent) {
        match event {
            ChatEvent::MessageReceived(msg) => {
                // Clear typing for the author.
                self.clear_typing(&msg.author.id);
                self.add_message(msg.clone());
            }
            ChatEvent::MessageEdited(msg) => {
                self.update_message(msg.clone());
            }
            ChatEvent::MessageDeleted {
                channel_id,
                message_id,
            } => {
                self.remove_message(channel_id, message_id);
            }
            ChatEvent::TypingStarted { channel_id, user } => {
                self.set_typing(channel_id, user.clone());
            }
            ChatEvent::PresenceChanged { user_id, status } => {
                // Update member presence if visible.
                if let Some(member) = self.members.iter_mut().find(|m| m.user.id == *user_id) {
                    member.presence = *status;
                }
            }
            ChatEvent::ChannelUpdated(channel) => {
                // Update the channel in its server.
                for server in &mut self.servers {
                    if let Some(ch) = server.channels.iter_mut().find(|c| c.id == channel.id) {
                        *ch = channel.clone();
                    }
                }
            }
            ChatEvent::Connected(_) | ChatEvent::Disconnected(_) | ChatEvent::Error { .. } => {
                // Connection state changes handled at a higher level.
            }
        }
    }

    /// Total unread count across all channels.
    #[must_use]
    pub fn total_unread(&self) -> u32 {
        self.servers
            .iter()
            .flat_map(|s| &s.channels)
            .map(|c| c.unread)
            .sum()
    }

    /// Total mention count across all channels.
    #[must_use]
    pub fn total_mentions(&self) -> u32 {
        self.servers
            .iter()
            .flat_map(|s| &s.channels)
            .map(|c| c.mention_count)
            .sum()
    }

    /// Find the channel info for the active channel.
    #[must_use]
    pub fn active_channel_info(&self) -> Option<&Channel> {
        let active_id = self.active_channel.as_ref()?;
        self.servers
            .iter()
            .flat_map(|s| &s.channels)
            .find(|c| c.id == *active_id)
    }

    /// Find the server info for the active server.
    #[must_use]
    pub fn active_server_info(&self) -> Option<&Server> {
        let active_id = self.active_server.as_ref()?;
        self.servers.iter().find(|s| s.id == *active_id)
    }
}

impl Default for UnifiedStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_display() {
        assert_eq!(Protocol::Discord.to_string(), "discord");
        assert_eq!(Protocol::Matrix.to_string(), "matrix");
        assert_eq!(Protocol::Slack.to_string(), "slack");
    }

    #[test]
    fn protocol_serde_roundtrip() {
        let protocols = [Protocol::Discord, Protocol::Matrix, Protocol::Slack];
        for p in &protocols {
            let json = serde_json::to_string(p).expect("serialize");
            let back: Protocol = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*p, back);
        }
    }

    #[test]
    fn message_serde_roundtrip() {
        let msg = Message {
            id: "msg-1".into(),
            protocol: Protocol::Discord,
            channel_id: "ch-1".into(),
            author: User {
                id: "u-1".into(),
                name: "alice".into(),
                display_name: Some("Alice".into()),
                avatar_url: None,
                protocol: Protocol::Discord,
                bot: false,
            },
            content: "hello world".into(),
            timestamp: 1_700_000_000,
            edited: false,
            attachments: vec![Attachment {
                filename: "pic.png".into(),
                url: "https://cdn.example.com/pic.png".into(),
                content_type: Some("image/png".into()),
                size: Some(12345),
            }],
            reactions: vec![Reaction {
                emoji: "\u{1f44d}".into(),
                count: 3,
                me: true,
            }],
            reply_to: None,
        };

        let json = serde_json::to_string(&msg).expect("serialize");
        let back: Message = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.id, "msg-1");
        assert_eq!(back.protocol, Protocol::Discord);
        assert_eq!(back.author.name, "alice");
        assert_eq!(back.attachments.len(), 1);
        assert_eq!(back.reactions[0].count, 3);
    }

    #[test]
    fn server_with_channels() {
        let server = Server {
            id: "srv-1".into(),
            name: "My Server".into(),
            protocol: Protocol::Discord,
            icon_url: None,
            channels: vec![
                Channel {
                    id: "ch-1".into(),
                    name: "general".into(),
                    protocol: Protocol::Discord,
                    channel_type: ChannelType::Text,
                    server_id: Some("srv-1".into()),
                    topic: None,
                    unread: 5,
                    mention_count: 1,
                },
                Channel {
                    id: "ch-2".into(),
                    name: "voice-lobby".into(),
                    protocol: Protocol::Discord,
                    channel_type: ChannelType::Voice,
                    server_id: Some("srv-1".into()),
                    topic: None,
                    unread: 0,
                    mention_count: 0,
                },
            ],
        };

        let json = serde_json::to_string(&server).expect("serialize");
        let back: Server = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.channels.len(), 2);
        assert_eq!(back.channels[0].channel_type, ChannelType::Text);
        assert_eq!(back.channels[1].channel_type, ChannelType::Voice);
    }

    #[test]
    fn channel_type_equality() {
        assert_eq!(ChannelType::Text, ChannelType::Text);
        assert_ne!(ChannelType::Text, ChannelType::Voice);
        assert_ne!(ChannelType::Direct, ChannelType::Group);
    }

    #[test]
    fn presence_status_equality() {
        assert_eq!(PresenceStatus::Online, PresenceStatus::Online);
        assert_ne!(PresenceStatus::Online, PresenceStatus::Offline);
        assert_ne!(PresenceStatus::Idle, PresenceStatus::DoNotDisturb);
    }

    #[test]
    fn chat_error_display() {
        let err = ChatError::Connection("timeout".into());
        assert_eq!(err.to_string(), "connection failed: timeout");

        let err = ChatError::Auth("bad token".into());
        assert_eq!(err.to_string(), "authentication failed: bad token");

        let err = ChatError::NotConnected;
        assert_eq!(err.to_string(), "not connected");
    }

    #[test]
    fn chat_event_variants() {
        let user = User {
            id: "u-1".into(),
            name: "bob".into(),
            display_name: None,
            avatar_url: None,
            protocol: Protocol::Matrix,
            bot: false,
        };

        let msg = Message {
            id: "m-1".into(),
            protocol: Protocol::Matrix,
            channel_id: "room-1".into(),
            author: user.clone(),
            content: "hi".into(),
            timestamp: 0,
            edited: false,
            attachments: vec![],
            reactions: vec![],
            reply_to: None,
        };

        let events: Vec<ChatEvent> = vec![
            ChatEvent::MessageReceived(msg.clone()),
            ChatEvent::MessageEdited(msg),
            ChatEvent::MessageDeleted {
                channel_id: "room-1".into(),
                message_id: "m-1".into(),
            },
            ChatEvent::TypingStarted {
                channel_id: "room-1".into(),
                user,
            },
            ChatEvent::PresenceChanged {
                user_id: "u-1".into(),
                status: PresenceStatus::Online,
            },
            ChatEvent::ChannelUpdated(Channel {
                id: "room-1".into(),
                name: "general".into(),
                protocol: Protocol::Matrix,
                channel_type: ChannelType::Text,
                server_id: None,
                topic: None,
                unread: 0,
                mention_count: 0,
            }),
            ChatEvent::Connected(Protocol::Matrix),
            ChatEvent::Disconnected(Protocol::Matrix),
            ChatEvent::Error {
                protocol: Protocol::Matrix,
                message: "oops".into(),
            },
        ];

        assert_eq!(events.len(), 9);
    }

    #[test]
    fn message_with_reply() {
        let msg = Message {
            id: "m-2".into(),
            protocol: Protocol::Slack,
            channel_id: "C123".into(),
            author: User {
                id: "U456".into(),
                name: "charlie".into(),
                display_name: Some("Charlie".into()),
                avatar_url: None,
                protocol: Protocol::Slack,
                bot: false,
            },
            content: "replying here".into(),
            timestamp: 1_700_000_100,
            edited: true,
            attachments: vec![],
            reactions: vec![],
            reply_to: Some("m-1".into()),
        };

        assert!(msg.edited);
        assert_eq!(msg.reply_to.as_deref(), Some("m-1"));
    }

    #[test]
    fn user_effective_name() {
        let user_with_display = User {
            id: "u1".into(),
            name: "alice".into(),
            display_name: Some("Alice W.".into()),
            avatar_url: None,
            protocol: Protocol::Discord,
            bot: false,
        };
        assert_eq!(user_with_display.effective_name(), "Alice W.");

        let user_without_display = User {
            id: "u2".into(),
            name: "bob".into(),
            display_name: None,
            avatar_url: None,
            protocol: Protocol::Discord,
            bot: false,
        };
        assert_eq!(user_without_display.effective_name(), "bob");
    }

    #[test]
    fn unified_store_merge_servers() {
        let mut store = UnifiedStore::new();
        let servers = vec![Server {
            id: "s1".into(),
            name: "Test".into(),
            protocol: Protocol::Discord,
            icon_url: None,
            channels: vec![],
        }];
        store.merge_servers(Protocol::Discord, &servers);
        assert_eq!(store.servers().len(), 1);

        // Merging again replaces.
        let servers2 = vec![
            Server {
                id: "s1".into(),
                name: "Test".into(),
                protocol: Protocol::Discord,
                icon_url: None,
                channels: vec![],
            },
            Server {
                id: "s2".into(),
                name: "Test2".into(),
                protocol: Protocol::Discord,
                icon_url: None,
                channels: vec![],
            },
        ];
        store.merge_servers(Protocol::Discord, &servers2);
        assert_eq!(store.servers().len(), 2);
    }

    #[test]
    fn unified_store_messages() {
        let mut store = UnifiedStore::new();
        let msg = Message {
            id: "m1".into(),
            protocol: Protocol::Discord,
            channel_id: "ch1".into(),
            author: User {
                id: "u1".into(),
                name: "alice".into(),
                display_name: None,
                avatar_url: None,
                protocol: Protocol::Discord,
                bot: false,
            },
            content: "hello".into(),
            timestamp: 1000,
            edited: false,
            attachments: vec![],
            reactions: vec![],
            reply_to: None,
        };
        store.add_message(msg);
        assert_eq!(store.channel_messages("ch1").len(), 1);
        assert_eq!(store.channel_messages("ch2").len(), 0);

        store.set_active_channel("ch1");
        assert_eq!(store.active_messages().len(), 1);
    }

    #[test]
    fn unified_store_handle_event() {
        let mut store = UnifiedStore::new();
        store.set_active_channel("ch1");

        let msg = Message {
            id: "m1".into(),
            protocol: Protocol::Discord,
            channel_id: "ch1".into(),
            author: User {
                id: "u1".into(),
                name: "alice".into(),
                display_name: None,
                avatar_url: None,
                protocol: Protocol::Discord,
                bot: false,
            },
            content: "hello".into(),
            timestamp: 1000,
            edited: false,
            attachments: vec![],
            reactions: vec![],
            reply_to: None,
        };

        store.handle_event(&ChatEvent::MessageReceived(msg));
        assert_eq!(store.active_messages().len(), 1);

        store.handle_event(&ChatEvent::MessageDeleted {
            channel_id: "ch1".into(),
            message_id: "m1".into(),
        });
        assert_eq!(store.active_messages().len(), 0);
    }
}
