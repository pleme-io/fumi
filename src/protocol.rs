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

/// A channel (text channel, DM, room, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub channel_type: ChannelType,
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

    /// Subscribe to real-time events. Returns a broadcast receiver that
    /// yields [`ChatEvent`] values.
    fn events(&self) -> tokio::sync::broadcast::Receiver<ChatEvent>;

    /// Protocol identifier for this backend.
    fn protocol(&self) -> Protocol;
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
                    unread: 5,
                    mention_count: 1,
                },
                Channel {
                    id: "ch-2".into(),
                    name: "voice-lobby".into(),
                    protocol: Protocol::Discord,
                    channel_type: ChannelType::Voice,
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
        // Ensure all event variants can be constructed.
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
}
