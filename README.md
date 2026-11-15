# Rustcord

A native Discord client written in Rust. It uses egui for the UI, tokio for async networking, and talks to Discord's REST API (v10) and Gateway directly, with no Electron or web runtime.

## Status

| Feature | Status |
|---|---|
| Connect with a bot token | Working |
| Server (guild) list and channel list with categories | Working |
| Read text channel history (last 50 messages) | Working |
| Live messages via the Gateway (MESSAGE_CREATE) | Working |
| Send messages | Working |
| Auto-reconnect and heartbeats | Working |
| Voice channels, calling, mic mute, deafen, camera | **Not implemented** (buttons are placeholders) |
| Video, screen share, reactions, threads, DMs, friends, attachments, embeds, markdown rendering | **Not implemented** |
| User settings, roles and permissions UI | **Not implemented** |

Full Discord feature parity is a large project. Voice and video need Discord's voice gateway, WebRTC media transport and its end-to-end encryption (DAVE). These are the next big milestones.

## How you connect

Rustcord has two ways in:

- **Rustcord account (default):** sign in to a Rustcord relay server with a username and password. The relay holds the Discord bot token, so users never need one. Servers and messages go through the relay, and messages are posted as your name and picture.
- **Discord bot token (advanced):** connect straight to Discord with your own bot token from the [Discord Developer Portal](https://discord.com/developers/applications). Enable the **Message Content Intent** and invite the bot to your server.

Discord's Terms of Service do not allow third-party clients to log in with a personal user token, so Rustcord doesn't support that.

## Build and run

```bash
cargo run --release
```

To build a macOS app bundle:

```bash
./packaging/macos/bundle.sh
```

Requires a recent stable Rust toolchain (edition 2021).

## Layout

- `src/main.rs`: window setup; starts the tokio network runtime on its own thread
- `src/app.rs`: egui UI (login, servers, channels, chat, composer)
- `src/net.rs`: bridge between the UI thread and the network tasks
- `src/gateway.rs`: Gateway WebSocket (Hello, Identify, heartbeats, dispatch, reconnect)
- `src/rest.rs`: REST client (messages, send)
- `src/models.rs`: Discord data types
