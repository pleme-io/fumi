# Fumi (文)

GPU-rendered multi-protocol chat client. One interface for Discord, Matrix, and Slack.

## Features

- GPU-accelerated chat UI via garasu + egaku widgets
- Rich text message rendering (markdown, embeds, reactions) via fude
- Voice chat via oto
- Multi-protocol: Discord (serenity), Matrix (matrix-sdk), Slack (REST+WebSocket)
- E2E encryption for Matrix rooms
- Desktop notifications
- Daemon mode for persistent connections
- Hot-reloadable configuration via shikumi

## Architecture

| Module | Purpose |
|--------|---------|
| `protocol` | Common trait: `ChatProtocol`, `Channel`, `Message`, `User` |
| `discord` | Discord backend via serenity |
| `matrix` | Matrix backend via matrix-sdk (E2E encrypted) |
| `slack` | Slack backend via REST + Socket Mode |
| `render` | GPU chat UI via garasu + egaku + fude |
| `config` | shikumi-based multi-account configuration |

## Shared Libraries

- **garasu** — GPU rendering engine
- **egaku** — UI widgets (text input, message list, sidebar, tabs)
- **fude** — rich text rendering (chat markdown, embeds)
- **oto** — voice chat (capture, playback, mute/deafen)
- **tsunagu** — daemon IPC (persistent connections)
- **shikumi** — config discovery + hot-reload

## Build

```bash
cargo build
cargo run
cargo run -- daemon
cargo run -- send --protocol discord --channel general "hello"
```

## Configuration

`~/.config/fumi/fumi.yaml`

```yaml
accounts:
  discord:
    token: "your-discord-token"
  matrix:
    homeserver: "https://matrix.org"
    username: "@you:matrix.org"
    token: "your-matrix-token"
  slack:
    - workspace: "myteam"
      token: "xoxb-your-slack-token"
notifications:
  enabled: true
  sound: true
voice:
  echo_cancellation: true
  noise_suppression: true
```
