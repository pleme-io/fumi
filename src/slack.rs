//! Slack protocol backend via REST API + Socket Mode.
//!
//! Uses [reqwest] for Slack Web API calls (sending messages, fetching history,
//! listing channels/users) and [tokio-tungstenite] for Socket Mode WebSocket
//! (receiving real-time events without a public HTTP endpoint).
//!
//! Maps Slack's data model to the common [`crate::protocol`] types:
//! - Workspace -> [`Server`]
//! - Channels/DMs -> [`Channel`]
//! - Messages -> [`Message`]
//! - Socket Mode events -> [`ChatEvent`]

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};

use crate::protocol::{
    Attachment, Channel, ChannelType, ChatBackend, ChatError, ChatEvent, Member, Message,
    PresenceStatus, Protocol, Reaction, Server, User,
};

/// Broadcast channel capacity for Slack events.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Slack Web API base URL.
const SLACK_API_BASE: &str = "https://slack.com/api";

// ---------------------------------------------------------------------------
// Slack API response types (subset)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SlackApiResponse<T> {
    ok: bool,
    error: Option<String>,
    #[serde(flatten)]
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct AuthTestResponse {
    user_id: Option<String>,
    user: Option<String>,
    team_id: Option<String>,
    team: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConversationsListData {
    channels: Option<Vec<SlackChannel>>,
}

#[derive(Debug, Deserialize)]
struct SlackChannel {
    id: String,
    name: Option<String>,
    is_channel: Option<bool>,
    is_group: Option<bool>,
    is_im: Option<bool>,
    is_mpim: Option<bool>,
    num_members: Option<u32>,
    topic: Option<SlackTopic>,
}

#[derive(Debug, Deserialize)]
struct SlackTopic {
    value: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConversationsHistoryData {
    messages: Option<Vec<SlackMessage>>,
}

#[derive(Debug, Deserialize)]
struct SlackMessage {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    ts: Option<String>,
    user: Option<String>,
    text: Option<String>,
    edited: Option<serde_json::Value>,
    thread_ts: Option<String>,
    reply_count: Option<u32>,
    files: Option<Vec<SlackFile>>,
    reactions: Option<Vec<SlackReaction>>,
}

#[derive(Debug, Deserialize)]
struct SlackFile {
    id: String,
    name: Option<String>,
    url_private: Option<String>,
    mimetype: Option<String>,
    size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SlackReaction {
    name: String,
    count: u32,
    users: Vec<String>,
}

#[derive(Debug, Serialize)]
struct PostMessageRequest<'a> {
    channel: &'a str,
    text: &'a str,
}

#[derive(Debug, Deserialize)]
struct PostMessageData {
    ts: Option<String>,
    channel: Option<String>,
    message: Option<SlackMessage>,
}

#[derive(Debug, Deserialize)]
struct ConnectionsOpenData {
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConversationsMembersData {
    members: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct UsersInfoData {
    user: Option<SlackUser>,
}

#[derive(Debug, Deserialize)]
struct SlackUser {
    id: String,
    name: Option<String>,
    real_name: Option<String>,
    profile: Option<SlackProfile>,
    is_bot: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct SlackProfile {
    display_name: Option<String>,
    image_48: Option<String>,
}

/// Socket Mode envelope from Slack.
#[derive(Debug, Deserialize)]
struct SocketModeEnvelope {
    envelope_id: Option<String>,
    #[serde(rename = "type")]
    envelope_type: Option<String>,
    payload: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// Slack backend implementation.
///
/// Each instance represents a single Slack workspace connection.
pub struct SlackBackend {
    token: String,
    app_token: Option<String>,
    workspace_name: String,
    servers: Vec<Server>,
    event_tx: broadcast::Sender<ChatEvent>,
    connected: bool,
    /// HTTP client for Slack Web API calls.
    http: reqwest::Client,
    /// Our own user ID (populated on connect via auth.test).
    self_user_id: Option<String>,
    /// Handle to the Socket Mode WebSocket task.
    socket_handle: Option<tokio::task::JoinHandle<()>>,
    /// Shared server state.
    shared_servers: Arc<RwLock<Vec<Server>>>,
}

impl SlackBackend {
    /// Create a new Slack backend for the given workspace.
    ///
    /// `token` should be a Slack Bot Token (`xoxb-...`) or User Token
    /// (`xoxp-...`) with the appropriate scopes.
    ///
    /// `app_token` is an optional App-Level Token (`xapp-...`) required
    /// for Socket Mode real-time events.
    pub fn new(workspace_name: &str, token: &str) -> Self {
        Self::with_app_token(workspace_name, token, None)
    }

    /// Create with an explicit app-level token for Socket Mode.
    pub fn with_app_token(workspace_name: &str, token: &str, app_token: Option<&str>) -> Self {
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .expect("valid auth header"),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("build reqwest client");

        Self {
            token: token.to_owned(),
            app_token: app_token.map(str::to_owned),
            workspace_name: workspace_name.to_owned(),
            servers: Vec::new(),
            event_tx,
            connected: false,
            http,
            self_user_id: None,
            socket_handle: None,
            shared_servers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Call a Slack Web API method (GET).
    async fn api_get<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: &[(&str, &str)],
    ) -> Result<T, ChatError> {
        let url = format!("{SLACK_API_BASE}/{method}");
        let resp = self
            .http
            .get(&url)
            .query(params)
            .send()
            .await
            .map_err(|e| ChatError::Api(e.to_string()))?;

        let api_resp: SlackApiResponse<T> = resp
            .json()
            .await
            .map_err(|e| ChatError::Api(e.to_string()))?;

        if !api_resp.ok {
            return Err(ChatError::Api(
                api_resp.error.unwrap_or_else(|| "unknown error".into()),
            ));
        }

        api_resp
            .data
            .ok_or_else(|| ChatError::Api("empty response data".into()))
    }

    /// Call a Slack Web API method (POST with JSON body).
    async fn api_post<T: serde::de::DeserializeOwned, B: Serialize>(
        &self,
        method: &str,
        body: &B,
    ) -> Result<T, ChatError> {
        let url = format!("{SLACK_API_BASE}/{method}");
        let resp = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| ChatError::Api(e.to_string()))?;

        let api_resp: SlackApiResponse<T> = resp
            .json()
            .await
            .map_err(|e| ChatError::Api(e.to_string()))?;

        if !api_resp.ok {
            return Err(ChatError::Api(
                api_resp.error.unwrap_or_else(|| "unknown error".into()),
            ));
        }

        api_resp
            .data
            .ok_or_else(|| ChatError::Api("empty response data".into()))
    }

    /// Call a Slack Web API method (POST) using the app token for Socket Mode.
    async fn api_post_app_token<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
    ) -> Result<T, ChatError> {
        let app_token = self
            .app_token
            .as_ref()
            .ok_or_else(|| ChatError::Connection("app token required for Socket Mode".into()))?;

        let url = format!("{SLACK_API_BASE}/{method}");
        let resp = reqwest::Client::new()
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {app_token}"))
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .send()
            .await
            .map_err(|e| ChatError::Api(e.to_string()))?;

        let api_resp: SlackApiResponse<T> = resp
            .json()
            .await
            .map_err(|e| ChatError::Api(e.to_string()))?;

        if !api_resp.ok {
            return Err(ChatError::Api(
                api_resp.error.unwrap_or_else(|| "unknown error".into()),
            ));
        }

        api_resp
            .data
            .ok_or_else(|| ChatError::Api("empty response data".into()))
    }

    /// Start Socket Mode WebSocket for real-time events.
    async fn start_socket_mode(&mut self) -> Result<(), ChatError> {
        // Only start if we have an app token.
        let Some(_app_token) = &self.app_token else {
            tracing::info!("no app token provided, Socket Mode disabled (polling only)");
            return Ok(());
        };

        let data: ConnectionsOpenData = self.api_post_app_token("apps.connections.open").await?;

        let ws_url = data
            .url
            .ok_or_else(|| ChatError::Connection("no WebSocket URL in response".into()))?;

        let event_tx = self.event_tx.clone();
        let self_user_id = self.self_user_id.clone().unwrap_or_default();

        self.socket_handle = Some(tokio::spawn(async move {
            tracing::info!("connecting to Slack Socket Mode WebSocket");

            let connect_result = tokio_tungstenite::connect_async(&ws_url).await;
            let (mut ws, _) = match connect_result {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::error!("Slack Socket Mode connection failed: {e}");
                    return;
                }
            };

            tracing::info!("Slack Socket Mode connected");

            while let Some(msg_result) = ws.next().await {
                let msg = match msg_result {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::error!("Slack WebSocket error: {e}");
                        break;
                    }
                };

                if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                    if let Ok(envelope) = serde_json::from_str::<SocketModeEnvelope>(&text) {
                        // Acknowledge the envelope.
                        if let Some(envelope_id) = &envelope.envelope_id {
                            let ack = serde_json::json!({"envelope_id": envelope_id});
                            let _ = ws
                                .send(tokio_tungstenite::tungstenite::Message::Text(
                                    ack.to_string().into(),
                                ))
                                .await;
                        }

                        // Process event payloads.
                        if envelope.envelope_type.as_deref() == Some("events_api") {
                            if let Some(payload) = &envelope.payload {
                                if let Some(event) = payload.get("event") {
                                    let event_type =
                                        event.get("type").and_then(|t| t.as_str());

                                    match event_type {
                                        Some("message") => {
                                            if let (Some(channel), Some(ts), Some(text_val)) = (
                                                event.get("channel").and_then(|c| c.as_str()),
                                                event.get("ts").and_then(|t| t.as_str()),
                                                event.get("text").and_then(|t| t.as_str()),
                                            ) {
                                                let user_id = event
                                                    .get("user")
                                                    .and_then(|u| u.as_str())
                                                    .unwrap_or("unknown")
                                                    .to_owned();
                                                let msg = Message {
                                                    id: ts.to_owned(),
                                                    protocol: Protocol::Slack,
                                                    channel_id: channel.to_owned(),
                                                    author: User {
                                                        id: user_id.clone(),
                                                        name: user_id,
                                                        display_name: None,
                                                        avatar_url: None,
                                                        protocol: Protocol::Slack,
                                                        bot: false,
                                                    },
                                                    content: text_val.to_owned(),
                                                    timestamp: slack_ts_to_unix(ts),
                                                    edited: false,
                                                    attachments: vec![],
                                                    reactions: vec![],
                                                    reply_to: event
                                                        .get("thread_ts")
                                                        .and_then(|t| t.as_str())
                                                        .map(str::to_owned),
                                                };
                                                let _ = event_tx
                                                    .send(ChatEvent::MessageReceived(msg));
                                            }
                                        }
                                        Some("message_changed") => {
                                            // Edited message event.
                                            if let Some(message_obj) =
                                                event.get("message")
                                            {
                                                if let (Some(channel), Some(ts), Some(text_val)) = (
                                                    event.get("channel").and_then(|c| c.as_str()),
                                                    message_obj
                                                        .get("ts")
                                                        .and_then(|t| t.as_str()),
                                                    message_obj
                                                        .get("text")
                                                        .and_then(|t| t.as_str()),
                                                ) {
                                                    let user_id = message_obj
                                                        .get("user")
                                                        .and_then(|u| u.as_str())
                                                        .unwrap_or("unknown")
                                                        .to_owned();
                                                    let msg = Message {
                                                        id: ts.to_owned(),
                                                        protocol: Protocol::Slack,
                                                        channel_id: channel.to_owned(),
                                                        author: User {
                                                            id: user_id.clone(),
                                                            name: user_id,
                                                            display_name: None,
                                                            avatar_url: None,
                                                            protocol: Protocol::Slack,
                                                            bot: false,
                                                        },
                                                        content: text_val.to_owned(),
                                                        timestamp: slack_ts_to_unix(ts),
                                                        edited: true,
                                                        attachments: vec![],
                                                        reactions: vec![],
                                                        reply_to: None,
                                                    };
                                                    let _ = event_tx
                                                        .send(ChatEvent::MessageEdited(msg));
                                                }
                                            }
                                        }
                                        Some("message_deleted") => {
                                            if let (Some(channel), Some(ts)) = (
                                                event.get("channel").and_then(|c| c.as_str()),
                                                event
                                                    .get("deleted_ts")
                                                    .and_then(|t| t.as_str()),
                                            ) {
                                                let _ = event_tx.send(
                                                    ChatEvent::MessageDeleted {
                                                        channel_id: channel.to_owned(),
                                                        message_id: ts.to_owned(),
                                                    },
                                                );
                                            }
                                        }
                                        Some("user_typing") => {
                                            if let (Some(channel), Some(user_id)) = (
                                                event.get("channel").and_then(|c| c.as_str()),
                                                event.get("user").and_then(|u| u.as_str()),
                                            ) {
                                                let user = User {
                                                    id: user_id.to_owned(),
                                                    name: user_id.to_owned(),
                                                    display_name: None,
                                                    avatar_url: None,
                                                    protocol: Protocol::Slack,
                                                    bot: false,
                                                };
                                                let _ = event_tx.send(
                                                    ChatEvent::TypingStarted {
                                                        channel_id: channel.to_owned(),
                                                        user,
                                                    },
                                                );
                                            }
                                        }
                                        Some("presence_change") => {
                                            if let (Some(user_id), Some(presence)) = (
                                                event.get("user").and_then(|u| u.as_str()),
                                                event
                                                    .get("presence")
                                                    .and_then(|p| p.as_str()),
                                            ) {
                                                let status = match presence {
                                                    "active" => PresenceStatus::Online,
                                                    "away" => PresenceStatus::Idle,
                                                    _ => PresenceStatus::Offline,
                                                };
                                                let _ = event_tx.send(
                                                    ChatEvent::PresenceChanged {
                                                        user_id: user_id.to_owned(),
                                                        status,
                                                    },
                                                );
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            }

            tracing::info!("Slack Socket Mode disconnected");
            let _ = self_user_id;
        }));

        Ok(())
    }
}

impl ChatBackend for SlackBackend {
    async fn connect(&mut self) -> Result<(), ChatError> {
        if self.connected {
            return Ok(());
        }

        tracing::info!(workspace = %self.workspace_name, "connecting to Slack");

        // 1. Verify token via auth.test.
        let auth: AuthTestResponse = self.api_get("auth.test", &[]).await?;
        let team_id = auth.team_id.unwrap_or_default();
        let team_name = auth.team.unwrap_or_else(|| self.workspace_name.clone());
        self.self_user_id = auth.user_id.clone();

        tracing::info!(
            user = ?auth.user,
            team = %team_name,
            "Slack auth successful"
        );

        // 2. Fetch channel list.
        let convos: ConversationsListData = self
            .api_get(
                "conversations.list",
                &[("types", "public_channel,private_channel,mpim,im"), ("limit", "200")],
            )
            .await?;

        let channels: Vec<Channel> = convos
            .channels
            .unwrap_or_default()
            .iter()
            .map(slack_channel_to_protocol)
            .collect();

        self.servers = vec![Server {
            id: team_id,
            name: team_name,
            protocol: Protocol::Slack,
            icon_url: None,
            channels,
        }];

        // 3. Start Socket Mode WebSocket for real-time events.
        self.start_socket_mode().await?;

        self.connected = true;
        let _ = self.event_tx.send(ChatEvent::Connected(Protocol::Slack));
        tracing::info!(workspace = %self.workspace_name, "Slack backend connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), ChatError> {
        if !self.connected {
            return Ok(());
        }

        tracing::info!(workspace = %self.workspace_name, "disconnecting from Slack");

        if let Some(handle) = self.socket_handle.take() {
            handle.abort();
        }

        self.connected = false;
        let _ = self
            .event_tx
            .send(ChatEvent::Disconnected(Protocol::Slack));
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
        if !self.connected {
            return Err(ChatError::NotConnected);
        }

        let body = PostMessageRequest {
            channel: channel_id,
            text: content,
        };

        let data: PostMessageData = self.api_post("chat.postMessage", &body).await?;

        let ts = data.ts.unwrap_or_default();
        let self_id = self.self_user_id.clone().unwrap_or_default();

        Ok(Message {
            id: ts.clone(),
            protocol: Protocol::Slack,
            channel_id: data.channel.unwrap_or_else(|| channel_id.to_owned()),
            author: User {
                id: self_id.clone(),
                name: self_id,
                display_name: None,
                avatar_url: None,
                protocol: Protocol::Slack,
                bot: false,
            },
            content: content.to_owned(),
            timestamp: slack_ts_to_unix(&ts),
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
        before: Option<&str>,
    ) -> Result<Vec<Message>, ChatError> {
        if !self.connected {
            return Err(ChatError::NotConnected);
        }

        let limit_str = limit.to_string();
        let mut params: Vec<(&str, &str)> =
            vec![("channel", channel_id), ("limit", &limit_str)];

        // `before` in Slack is the `latest` timestamp parameter.
        if let Some(ts) = before {
            params.push(("latest", ts));
        }

        let data: ConversationsHistoryData =
            self.api_get("conversations.history", &params).await?;

        let messages = data
            .messages
            .unwrap_or_default()
            .into_iter()
            .map(|m| slack_message_to_protocol(m, channel_id))
            .collect();

        Ok(messages)
    }

    async fn list_members(
        &self,
        channel_id: &str,
    ) -> Result<Vec<Member>, ChatError> {
        if !self.connected {
            return Err(ChatError::NotConnected);
        }

        let data: ConversationsMembersData = self
            .api_get("conversations.members", &[("channel", channel_id), ("limit", "100")])
            .await?;

        let member_ids = data.members.unwrap_or_default();

        // Fetch user info for each member. In production you'd batch this.
        let mut members = Vec::new();
        for user_id in member_ids.iter().take(50) {
            // Limit to avoid rate limits
            if let Ok(user_data) = self
                .api_get::<UsersInfoData>("users.info", &[("user", user_id)])
                .await
            {
                if let Some(slack_user) = user_data.user {
                    members.push(Member {
                        user: slack_user_to_protocol(&slack_user),
                        presence: PresenceStatus::Online,
                        role: None,
                    });
                }
            }
        }

        Ok(members)
    }

    fn events(&self) -> broadcast::Receiver<ChatEvent> {
        self.event_tx.subscribe()
    }

    fn protocol(&self) -> Protocol {
        Protocol::Slack
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a Slack channel API object to a protocol [`Channel`].
fn slack_channel_to_protocol(ch: &SlackChannel) -> Channel {
    let channel_type = if ch.is_im.unwrap_or(false) {
        ChannelType::Direct
    } else if ch.is_mpim.unwrap_or(false) {
        ChannelType::Group
    } else {
        ChannelType::Text
    };

    Channel {
        id: ch.id.clone(),
        name: ch.name.clone().unwrap_or_else(|| ch.id.clone()),
        protocol: Protocol::Slack,
        channel_type,
        server_id: None,
        topic: ch.topic.as_ref().and_then(|t| t.value.clone()),
        unread: 0,
        mention_count: 0,
    }
}

/// Convert a Slack message API object to a protocol [`Message`].
fn slack_message_to_protocol(msg: SlackMessage, channel_id: &str) -> Message {
    let ts = msg.ts.clone().unwrap_or_default();
    let user_id = msg.user.clone().unwrap_or_default();

    Message {
        id: ts.clone(),
        protocol: Protocol::Slack,
        channel_id: channel_id.to_owned(),
        author: User {
            id: user_id.clone(),
            name: user_id,
            display_name: None,
            avatar_url: None,
            protocol: Protocol::Slack,
            bot: false,
        },
        content: msg.text.unwrap_or_default(),
        timestamp: slack_ts_to_unix(&ts),
        edited: msg.edited.is_some(),
        attachments: msg
            .files
            .unwrap_or_default()
            .into_iter()
            .map(|f| Attachment {
                filename: f.name.unwrap_or_else(|| f.id),
                url: f.url_private.unwrap_or_default(),
                content_type: f.mimetype,
                size: f.size,
            })
            .collect(),
        reactions: msg
            .reactions
            .unwrap_or_default()
            .into_iter()
            .map(|r| Reaction {
                emoji: r.name,
                count: r.count,
                me: false, // Would need self_user_id to determine
            })
            .collect(),
        reply_to: msg.thread_ts,
    }
}

/// Convert a Slack user API object to a protocol [`User`].
fn slack_user_to_protocol(user: &SlackUser) -> User {
    User {
        id: user.id.clone(),
        name: user.name.clone().unwrap_or_else(|| user.id.clone()),
        display_name: user
            .profile
            .as_ref()
            .and_then(|p| p.display_name.clone())
            .or_else(|| user.real_name.clone()),
        avatar_url: user.profile.as_ref().and_then(|p| p.image_48.clone()),
        protocol: Protocol::Slack,
        bot: user.is_bot.unwrap_or(false),
    }
}

/// Convert a Slack timestamp string (e.g. "1700000000.000100") to a Unix
/// epoch in seconds.
fn slack_ts_to_unix(ts: &str) -> u64 {
    ts.split('.')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_new_defaults() {
        let backend = SlackBackend::new("test-workspace", "xoxb-test-token");
        assert!(!backend.connected);
        assert_eq!(backend.protocol(), Protocol::Slack);
        assert!(backend.servers().is_empty());
        assert!(!backend.is_connected());
    }

    #[test]
    fn event_subscription() {
        let backend = SlackBackend::new("ws", "xoxb-tok");
        let mut rx = backend.events();

        let _ = backend
            .event_tx
            .send(ChatEvent::Connected(Protocol::Slack));

        let event = rx.try_recv().expect("should receive event");
        match event {
            ChatEvent::Connected(p) => assert_eq!(p, Protocol::Slack),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn slack_ts_to_unix_basic() {
        assert_eq!(slack_ts_to_unix("1700000000.000100"), 1_700_000_000);
        assert_eq!(slack_ts_to_unix("1234567890.123456"), 1_234_567_890);
    }

    #[test]
    fn slack_ts_to_unix_invalid() {
        assert_eq!(slack_ts_to_unix(""), 0);
        assert_eq!(slack_ts_to_unix("not-a-number"), 0);
    }

    #[test]
    fn slack_channel_conversion() {
        let ch = SlackChannel {
            id: "C123".into(),
            name: Some("general".into()),
            is_channel: Some(true),
            is_group: Some(false),
            is_im: Some(false),
            is_mpim: Some(false),
            num_members: Some(42),
            topic: Some(SlackTopic { value: Some("Welcome!".into()) }),
        };

        let converted = slack_channel_to_protocol(&ch);
        assert_eq!(converted.id, "C123");
        assert_eq!(converted.name, "general");
        assert_eq!(converted.channel_type, ChannelType::Text);
        assert_eq!(converted.protocol, Protocol::Slack);
        assert_eq!(converted.topic.as_deref(), Some("Welcome!"));
    }

    #[test]
    fn slack_dm_channel_conversion() {
        let ch = SlackChannel {
            id: "D456".into(),
            name: None,
            is_channel: Some(false),
            is_group: Some(false),
            is_im: Some(true),
            is_mpim: Some(false),
            num_members: None,
            topic: None,
        };

        let converted = slack_channel_to_protocol(&ch);
        assert_eq!(converted.channel_type, ChannelType::Direct);
        assert_eq!(converted.name, "D456");
    }

    #[test]
    fn slack_group_dm_channel_conversion() {
        let ch = SlackChannel {
            id: "G789".into(),
            name: Some("mpdm-group".into()),
            is_channel: Some(false),
            is_group: Some(false),
            is_im: Some(false),
            is_mpim: Some(true),
            num_members: Some(3),
            topic: None,
        };

        let converted = slack_channel_to_protocol(&ch);
        assert_eq!(converted.channel_type, ChannelType::Group);
    }

    #[test]
    fn slack_message_conversion() {
        let msg = SlackMessage {
            msg_type: Some("message".into()),
            ts: Some("1700000000.000100".into()),
            user: Some("U123".into()),
            text: Some("hello world".into()),
            edited: None,
            thread_ts: None,
            reply_count: None,
            files: Some(vec![SlackFile {
                id: "F1".into(),
                name: Some("doc.pdf".into()),
                url_private: Some("https://files.slack.com/doc.pdf".into()),
                mimetype: Some("application/pdf".into()),
                size: Some(99999),
            }]),
            reactions: Some(vec![SlackReaction {
                name: "thumbsup".into(),
                count: 5,
                users: vec!["U1".into(), "U2".into()],
            }]),
        };

        let converted = slack_message_to_protocol(msg, "C123");
        assert_eq!(converted.id, "1700000000.000100");
        assert_eq!(converted.timestamp, 1_700_000_000);
        assert_eq!(converted.content, "hello world");
        assert!(!converted.edited);
        assert_eq!(converted.attachments.len(), 1);
        assert_eq!(converted.attachments[0].filename, "doc.pdf");
        assert_eq!(converted.reactions.len(), 1);
        assert_eq!(converted.reactions[0].emoji, "thumbsup");
        assert_eq!(converted.reactions[0].count, 5);
        assert!(converted.reply_to.is_none());
    }

    #[test]
    fn slack_edited_message_conversion() {
        let msg = SlackMessage {
            msg_type: Some("message".into()),
            ts: Some("1700000000.000200".into()),
            user: Some("U456".into()),
            text: Some("edited text".into()),
            edited: Some(serde_json::json!({"user": "U456", "ts": "1700000001.000000"})),
            thread_ts: Some("1700000000.000100".into()),
            reply_count: Some(3),
            files: None,
            reactions: None,
        };

        let converted = slack_message_to_protocol(msg, "C123");
        assert!(converted.edited);
        assert_eq!(converted.reply_to, Some("1700000000.000100".into()));
    }

    #[tokio::test]
    async fn send_message_without_connect_fails() {
        let backend = SlackBackend::new("ws", "xoxb-tok");
        let result = backend.send_message("C123", "hello").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ChatError::NotConnected => {}
            other => panic!("expected NotConnected, got: {other}"),
        }
    }

    #[tokio::test]
    async fn fetch_messages_without_connect_fails() {
        let backend = SlackBackend::new("ws", "xoxb-tok");
        let result = backend.fetch_messages("C123", 50, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn disconnect_when_not_connected() {
        let mut backend = SlackBackend::new("ws", "xoxb-tok");
        let result = backend.disconnect().await;
        assert!(result.is_ok());
    }

    #[test]
    fn slack_user_conversion() {
        let user = SlackUser {
            id: "U123".into(),
            name: Some("alice".into()),
            real_name: Some("Alice Smith".into()),
            profile: Some(SlackProfile {
                display_name: Some("Alice".into()),
                image_48: Some("https://avatars.slack.com/alice.png".into()),
            }),
            is_bot: Some(false),
        };

        let converted = slack_user_to_protocol(&user);
        assert_eq!(converted.id, "U123");
        assert_eq!(converted.name, "alice");
        assert_eq!(converted.display_name, Some("Alice".into()));
        assert_eq!(
            converted.avatar_url,
            Some("https://avatars.slack.com/alice.png".into())
        );
        assert!(!converted.bot);
    }
}
