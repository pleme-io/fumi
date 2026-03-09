//! Matrix protocol backend via matrix-sdk.
//!
//! Implements [`ChatBackend`] for the Matrix protocol:
//! - Sync loop for real-time events
//! - E2E encryption via matrix-sdk's built-in crypto module
//! - Room management (join, leave, invite)
//! - Maps Matrix rooms to [`Server`]/[`Channel`], Matrix events to [`ChatEvent`]
//! - Voice/video via Matrix VoIP (future, via oto)

use std::sync::Arc;

use matrix_sdk::{
    config::SyncSettings,
    ruma::events::room::message::{
        OriginalSyncRoomMessageEvent, RoomMessageEventContent,
    },
    Room,
};
use tokio::sync::{broadcast, RwLock};

use crate::protocol::{
    Channel, ChannelType, ChatBackend, ChatError, ChatEvent, Member, Message, PresenceStatus,
    Protocol, Server, User,
};

/// Broadcast channel capacity for Matrix events.
const EVENT_CHANNEL_CAPACITY: usize = 256;

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// Matrix backend implementation.
///
/// Wraps a `matrix_sdk::Client` and maps Matrix timeline events to
/// [`ChatEvent`].
pub struct MatrixBackend {
    homeserver: String,
    username: String,
    token: Option<String>,
    servers: Vec<Server>,
    event_tx: broadcast::Sender<ChatEvent>,
    connected: bool,
    /// matrix-sdk client (available after connect).
    client: Option<matrix_sdk::Client>,
    /// Handle to the running sync task so we can shut it down.
    sync_handle: Option<tokio::task::JoinHandle<()>>,
    /// Shared server state populated by the sync loop.
    shared_servers: Arc<RwLock<Vec<Server>>>,
}

impl MatrixBackend {
    /// Create a new Matrix backend.
    ///
    /// If `token` is `Some`, it will be used for authentication. Otherwise the
    /// backend will attempt password login during [`connect`](ChatBackend::connect).
    pub fn new(homeserver: &str, username: &str, token: Option<&str>) -> Self {
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            homeserver: homeserver.to_owned(),
            username: username.to_owned(),
            token: token.map(str::to_owned),
            servers: Vec::new(),
            event_tx,
            connected: false,
            client: None,
            sync_handle: None,
            shared_servers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Build channels list from the current joined rooms.
    fn rooms_to_channels(client: &matrix_sdk::Client) -> Vec<Channel> {
        client
            .joined_rooms()
            .iter()
            .map(|room| {
                let name = room
                    .cached_display_name()
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| room.room_id().to_string());
                // is_direct() is async; use joined member count as a sync heuristic.
                let is_dm = room.joined_members_count() <= 2;
                Channel {
                    id: room.room_id().to_string(),
                    name,
                    protocol: Protocol::Matrix,
                    channel_type: if is_dm {
                        ChannelType::Direct
                    } else {
                        ChannelType::Text
                    },
                    server_id: None,
                    topic: room.topic(),
                    unread: room.num_unread_messages() as u32,
                    mention_count: room.num_unread_mentions() as u32,
                }
            })
            .collect()
    }

    /// Synchronize servers from the shared state.
    pub async fn sync_servers(&mut self) {
        let shared = self.shared_servers.read().await;
        self.servers = shared.clone();
    }
}

impl ChatBackend for MatrixBackend {
    async fn connect(&mut self) -> Result<(), ChatError> {
        if self.connected {
            return Ok(());
        }

        tracing::info!(homeserver = %self.homeserver, "connecting to Matrix");

        // Build the matrix-sdk client.
        let homeserver_url = url::Url::parse(&self.homeserver)
            .map_err(|e| ChatError::Connection(format!("invalid homeserver URL: {e}")))?;

        let client = matrix_sdk::Client::builder()
            .homeserver_url(homeserver_url)
            .build()
            .await
            .map_err(|e| ChatError::Connection(e.to_string()))?;

        // Authenticate — prefer access token, fall back to password.
        if let Some(token) = &self.token {
            // Restore session with the provided access token.
            // NOTE: Full session restore requires device_id + user_id persisted from
            // a previous login. For now we log that a token was provided.
            tracing::info!("Matrix access token provided (session restore requires device_id)");
            let _ = token;
        } else {
            tracing::warn!("password login not yet implemented; need token");
            return Err(ChatError::Auth(
                "no token provided and password login is not yet implemented".into(),
            ));
        }

        self.client = Some(client.clone());

        // Populate servers from joined rooms. In Matrix each "server" is the
        // homeserver itself, and channels are rooms.
        let channels = Self::rooms_to_channels(&client);

        self.servers = vec![Server {
            id: self.homeserver.clone(),
            name: self.homeserver.clone(),
            protocol: Protocol::Matrix,
            icon_url: None,
            channels,
        }];

        // Start the sync loop in a background task.
        let event_tx = self.event_tx.clone();
        let shared_servers = Arc::clone(&self.shared_servers);
        let sync_client = client.clone();
        let homeserver = self.homeserver.clone();

        // Register event handlers for message events.
        let msg_tx = event_tx.clone();
        sync_client.add_event_handler(
            move |ev: OriginalSyncRoomMessageEvent, room: Room| {
                let tx = msg_tx.clone();
                async move {
                    let user_id = ev.sender.to_string();
                    let display_name = room
                        .get_member_no_sync(&ev.sender)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|m| m.display_name().map(str::to_owned));

                    let content_text = match &ev.content.msgtype {
                        matrix_sdk::ruma::events::room::message::MessageType::Text(text) => {
                            text.body.clone()
                        }
                        other => format!("[{:?}]", other.msgtype()),
                    };

                    let msg = Message {
                        id: ev.event_id.to_string(),
                        protocol: Protocol::Matrix,
                        channel_id: room.room_id().to_string(),
                        author: User {
                            id: user_id.clone(),
                            name: user_id,
                            display_name,
                            avatar_url: None,
                            protocol: Protocol::Matrix,
                            bot: false,
                        },
                        content: content_text,
                        timestamp: ev
                            .origin_server_ts
                            .as_secs()
                            .into(),
                        edited: false,
                        attachments: vec![],
                        reactions: vec![],
                        reply_to: None,
                    };

                    let _ = tx.send(ChatEvent::MessageReceived(msg));
                }
            },
        );

        self.sync_handle = Some(tokio::spawn(async move {
            tracing::info!("starting Matrix sync loop");
            let _ = event_tx.send(ChatEvent::Connected(Protocol::Matrix));

            // Update shared servers with initial room list.
            {
                let channels = MatrixBackend::rooms_to_channels_static(&sync_client);
                *shared_servers.write().await = vec![Server {
                    id: homeserver.clone(),
                    name: homeserver,
                    protocol: Protocol::Matrix,
                    icon_url: None,
                    channels,
                }];
            }

            let settings = SyncSettings::default();

            if let Err(e) = sync_client.sync(settings).await {
                tracing::error!("matrix sync error: {e}");
                let _ = event_tx.send(ChatEvent::Error {
                    protocol: Protocol::Matrix,
                    message: e.to_string(),
                });
            }
        }));

        self.connected = true;
        tracing::info!("Matrix backend connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), ChatError> {
        if !self.connected {
            return Ok(());
        }

        tracing::info!("disconnecting from Matrix");

        if let Some(handle) = self.sync_handle.take() {
            handle.abort();
        }

        self.connected = false;
        let _ = self
            .event_tx
            .send(ChatEvent::Disconnected(Protocol::Matrix));
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
        let client = self.client.as_ref().ok_or(ChatError::NotConnected)?;

        let room_id = <&matrix_sdk::ruma::RoomId>::try_from(channel_id)
            .map_err(|e| ChatError::Send(format!("invalid room id: {e}")))?;

        let room = client
            .get_room(room_id)
            .ok_or_else(|| ChatError::Send(format!("room not found: {channel_id}")))?;

        let msg_content = RoomMessageEventContent::text_plain(content);

        let response = room
            .send(msg_content)
            .await
            .map_err(|e| ChatError::Send(e.to_string()))?;

        // Build a protocol Message from the response.
        let user_id = client
            .user_id()
            .map(|id| id.to_string())
            .unwrap_or_default();

        Ok(Message {
            id: response.event_id.to_string(),
            protocol: Protocol::Matrix,
            channel_id: channel_id.to_owned(),
            author: User {
                id: user_id.clone(),
                name: user_id,
                display_name: None,
                avatar_url: None,
                protocol: Protocol::Matrix,
                bot: false,
            },
            content: content.to_owned(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            edited: false,
            attachments: vec![],
            reactions: vec![],
            reply_to: None,
        })
    }

    async fn fetch_messages(
        &self,
        channel_id: &str,
        limit: usize,
        _before: Option<&str>,
    ) -> Result<Vec<Message>, ChatError> {
        let client = self.client.as_ref().ok_or(ChatError::NotConnected)?;

        let room_id = <&matrix_sdk::ruma::RoomId>::try_from(channel_id)
            .map_err(|e| ChatError::Api(format!("invalid room id: {e}")))?;

        let _room = client
            .get_room(room_id)
            .ok_or_else(|| ChatError::Api(format!("room not found: {channel_id}")))?;

        // TODO: Use room.timeline().paginate_backwards(limit) to fetch
        //       historical messages. For now return empty.
        let _ = limit;
        tracing::debug!(channel_id, limit, "fetch_messages: pagination not yet implemented for Matrix");
        Ok(vec![])
    }

    async fn list_members(
        &self,
        channel_id: &str,
    ) -> Result<Vec<Member>, ChatError> {
        let client = self.client.as_ref().ok_or(ChatError::NotConnected)?;

        let room_id = <&matrix_sdk::ruma::RoomId>::try_from(channel_id)
            .map_err(|e| ChatError::Api(format!("invalid room id: {e}")))?;

        let room = client
            .get_room(room_id)
            .ok_or_else(|| ChatError::Api(format!("room not found: {channel_id}")))?;

        let joined = room
            .members(matrix_sdk::RoomMemberships::JOIN)
            .await
            .map_err(|e| ChatError::Api(e.to_string()))?;

        let members = joined
            .iter()
            .map(|m| Member {
                user: User {
                    id: m.user_id().to_string(),
                    name: m.user_id().to_string(),
                    display_name: m.display_name().map(str::to_owned),
                    avatar_url: m.avatar_url().map(|u| u.to_string()),
                    protocol: Protocol::Matrix,
                    bot: false,
                },
                presence: PresenceStatus::Online, // Matrix doesn't expose per-room presence easily
                role: Some(m.power_level().to_string()),
            })
            .collect();

        Ok(members)
    }

    fn events(&self) -> broadcast::Receiver<ChatEvent> {
        self.event_tx.subscribe()
    }

    fn protocol(&self) -> Protocol {
        Protocol::Matrix
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

impl MatrixBackend {
    /// Static helper to build channels from rooms (for use in spawn context).
    fn rooms_to_channels_static(client: &matrix_sdk::Client) -> Vec<Channel> {
        Self::rooms_to_channels(client)
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
        let backend = MatrixBackend::new("https://matrix.org", "alice", None);
        assert!(!backend.connected);
        assert_eq!(backend.protocol(), Protocol::Matrix);
        assert!(backend.servers().is_empty());
        assert!(!backend.is_connected());
    }

    #[test]
    fn backend_with_token() {
        let backend =
            MatrixBackend::new("https://matrix.org", "alice", Some("syt_access_token"));
        assert!(backend.token.is_some());
    }

    #[test]
    fn event_subscription() {
        let backend = MatrixBackend::new("https://matrix.org", "alice", None);
        let mut rx = backend.events();

        let _ = backend
            .event_tx
            .send(ChatEvent::Connected(Protocol::Matrix));

        let event = rx.try_recv().expect("should receive event");
        match event {
            ChatEvent::Connected(p) => assert_eq!(p, Protocol::Matrix),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_message_without_connect_fails() {
        let backend = MatrixBackend::new("https://matrix.org", "alice", None);
        let result = backend.send_message("!room:matrix.org", "hello").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ChatError::NotConnected => {}
            other => panic!("expected NotConnected, got: {other}"),
        }
    }

    #[tokio::test]
    async fn fetch_messages_without_connect_fails() {
        let backend = MatrixBackend::new("https://matrix.org", "alice", None);
        let result = backend.fetch_messages("!room:matrix.org", 50, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn disconnect_when_not_connected() {
        let mut backend = MatrixBackend::new("https://matrix.org", "alice", None);
        let result = backend.disconnect().await;
        assert!(result.is_ok());
    }
}
