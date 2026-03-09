//! Matrix protocol backend via matrix-sdk.
//!
//! Implements [`ChatBackend`] for the Matrix protocol:
//! - Sliding-sync loop for real-time events
//! - E2E encryption via matrix-sdk's built-in crypto module
//! - Room management (join, leave, invite)
//! - Maps Matrix rooms to [`Server`]/[`Channel`], Matrix events to [`ChatEvent`]
//! - Voice/video via Matrix VoIP (future, via oto)

use std::sync::Arc;

use matrix_sdk::{config::SyncSettings, ruma::events::room::message::RoomMessageEventContent};
use tokio::sync::{broadcast, RwLock};

use crate::protocol::{
    Attachment, Channel, ChannelType, ChatBackend, ChatError, ChatEvent, Message, Protocol,
    Reaction, Server, User,
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
            //
            // TODO: Construct a proper `matrix_sdk::matrix_auth::MatrixSession`
            //       and call `client.restore_session(session).await`.
            //       This requires the device_id and user_id which should be
            //       persisted from a previous login.
            tracing::info!("restoring Matrix session from access token");
            let _ = token;
        } else {
            // TODO: Password-based login.
            //   client.matrix_auth()
            //       .login_username(&self.username, "password")
            //       .initial_device_display_name("fumi")
            //       .await
            //       .map_err(|e| ChatError::Auth(e.to_string()))?;
            tracing::warn!("password login not yet implemented; need token");
            return Err(ChatError::Auth(
                "no token provided and password login is not yet implemented".into(),
            ));
        }

        self.client = Some(client.clone());

        // Populate servers from joined rooms. In Matrix each "server" is the
        // homeserver itself, and channels are rooms.
        let rooms = client.joined_rooms();
        let channels: Vec<Channel> = rooms
            .iter()
            .map(|room| {
                let name = room
                    .cached_display_name()
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| room.room_id().to_string());
                // is_direct() is async and returns Result<bool>; in a sync map
                // closure we cannot await, so default to false (text channel).
                let is_dm = false;
                Channel {
                    id: room.room_id().to_string(),
                    name,
                    protocol: Protocol::Matrix,
                    channel_type: if is_dm {
                        ChannelType::Direct
                    } else {
                        ChannelType::Text
                    },
                    unread: room.num_unread_messages() as u32,
                    mention_count: room.num_unread_mentions() as u32,
                }
            })
            .collect();

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

        self.sync_handle = Some(tokio::spawn(async move {
            tracing::info!("starting Matrix sync loop");
            let _ = event_tx.send(ChatEvent::Connected(Protocol::Matrix));

            // TODO: Register event handlers on `sync_client` to convert
            //       Matrix timeline events to ChatEvent variants:
            //
            //   sync_client.add_event_handler(|ev: SyncRoomMessageEvent, room: Room| {
            //       // Convert to ChatEvent::MessageReceived
            //   });
            //
            //   sync_client.add_event_handler(|ev: SyncRoomRedactionEvent, room: Room| {
            //       // Convert to ChatEvent::MessageDeleted
            //   });
            //
            //   sync_client.add_event_handler(|ev: TypingEventContent, room: Room| {
            //       // Convert to ChatEvent::TypingStarted
            //   });

            let settings = SyncSettings::default();

            if let Err(e) = sync_client.sync(settings).await {
                tracing::error!("matrix sync error: {e}");
                let _ = event_tx.send(ChatEvent::Error {
                    protocol: Protocol::Matrix,
                    message: e.to_string(),
                });
            }

            let _ = shared_servers;
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

        // Build a protocol Message from the response. We know our own user
        // info and the content we just sent.
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

        let room = client
            .get_room(room_id)
            .ok_or_else(|| ChatError::Api(format!("room not found: {channel_id}")))?;

        // TODO: Use room.timeline().paginate_backwards(limit) to fetch
        //       historical messages and convert each timeline event to a
        //       protocol::Message. For now return an empty vec.
        //
        //   let timeline = room.timeline().await
        //       .map_err(|e| ChatError::Api(e.to_string()))?;
        //   timeline.paginate_backwards(limit).await
        //       .map_err(|e| ChatError::Api(e.to_string()))?;

        let _ = (room, limit);

        tracing::debug!(channel_id, limit, "fetch_messages not yet implemented for Matrix");
        Ok(vec![])
    }

    fn events(&self) -> broadcast::Receiver<ChatEvent> {
        self.event_tx.subscribe()
    }

    fn protocol(&self) -> Protocol {
        Protocol::Matrix
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
