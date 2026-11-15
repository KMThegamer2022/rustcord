# Rustcord

A small, fast, native chat app written in Rust. It looks like Discord and shows your Discord servers, but **it is not a Discord client**. Rustcord is a **bot client**: it reads and writes your servers through a Discord bot, using Discord's official bot API.

Rustcord is an independent project and is not made by or affiliated with Discord.

## What it is

- **A Rust desktop app** (egui for the window, tokio for networking). There is no Electron or web view.
- **A bot client.** Discord only lets a bot see servers it has been invited to. Rustcord shows only those servers. It cannot see your personal Discord account, DMs, or servers the bot isn't in.
- **Account-based.** Users sign in to a Rustcord relay server with a Rustcord username and password. They don't need a Discord account or a bot token.

## How it works

```
Rustcord app  --HTTPS-->  Rustcord relay  --Discord bot API-->  Discord
(your Mac/PC)             (a small server)  (holds the bot token)
```

- The **relay** holds the Discord bot token, so people using Rustcord never see it. It handles accounts, profiles, which servers each person has joined, and messages.
- When you send a message, the relay posts it through a **Discord webhook** in that channel, using your Rustcord name and picture. Other people in the server see your name and picture on that message.
- A server can be added only if the **bot is already in it**. The relay checks this before letting anyone join.

Relay code lives in its own project. This repository is the app.

## What works

| Feature | Status |
|---|---|
| Create a Rustcord account and sign in | Working |
| Stay signed in between launches (saved in the macOS Keychain) | Working |
| Edit your display name, picture link, and description | Working |
| Add a Discord server by its ID (the bot must be in it) | Working |
| Channel list with categories | Working |
| Read the last 50 messages in a text channel | Working |
| Send messages (shown with your name and picture) | Working, needs the **Manage Webhooks** permission (see setup) |
| New messages appear automatically | Working (checks every few seconds) |
| Bot-token mode: live messages, send, edit and delete your own messages | Working |

## What doesn't work yet

- **Direct messages** with a Rustcord account. DMs only appear in bot-token mode.
- **Editing or deleting messages** sent through a Rustcord account.
- **Your picture on your own messages** is stored by the relay, but the app doesn't show it on messages yet.
- **Voice, video, screen share, calling, mute, deafen and camera.** The buttons are placeholders.
- **Reactions, threads, friends, embeds, markdown, and file uploads.**
- **Server settings, roles and permissions** inside Rustcord.
- **Full Discord parity.** Voice and video need Discord's voice system and encryption, and are a large separate project.

## Setup

### 1. Create the Discord bot

1. Go to the [Discord Developer Portal](https://discord.com/developers/applications) and click **New Application**.
2. Open **Bot**. Under **Privileged Gateway Intents**, turn on **Message Content Intent**.
3. Click **Reset Token** and copy the token. You'll need it for the relay. Never share it or commit it to a repository.
4. Open **OAuth2 → URL Generator**. Select the `bot` scope and these permissions: **View Channels**, **Send Messages**, **Read Message History**, **Manage Webhooks**. Open the link and invite the bot to your server.
5. In the server, check the bot's role has **Manage Webhooks** turned on. The bot needs it to post messages as users.

### 2. Run the relay

The relay is a small Node.js server. It can run on any host that runs Node.js 18 or newer (a VPS, or a shared host with a Node.js app feature). It needs one environment variable:

- `RUSTCORD_BOT_TOKEN`: the bot token from step 1

Start it with `node app.js`. It stores accounts in a `data.json` file next to it. Put it behind HTTPS, since the app only connects over HTTPS (except `localhost`).

### 3. Install the app

Build from source:

```bash
cargo run --release
```

Or build a macOS app bundle:

```bash
./packaging/macos/bundle.sh
```

The app connects to the relay address set in the sign-in screen. Enter your relay's address, then create an account or log in.

### 4. Add a server

1. In Discord, turn on **Developer Mode** (Settings → Advanced).
2. Right-click the server and choose **Copy Server ID**.
3. In Rustcord, click **+** and paste the ID. The bot must already be in that server.

## Bot-token mode (advanced)

On the sign-in screen, choose **Use a Discord bot token instead**, paste the token, and connect. Rustcord connects directly to Discord, with live messages and editing and deleting your own messages. This mode doesn't use the relay, so messages are posted as the bot, not as individual users.

## Build and layout

Requires a recent stable Rust toolchain (edition 2021).

- `src/main.rs`: window setup; runs the network runtime on its own thread
- `src/app.rs`: the interface (sign-in, servers, channels, chat, profile, add-server)
- `src/relay.rs`: client for the relay server (accounts, profiles, servers, messages)
- `src/net.rs`: bridge between the interface and the Discord connection
- `src/gateway.rs`: Discord Gateway connection (live messages, heartbeats, reconnects)
- `src/rest.rs`: Discord REST calls (bot-token mode)
- `src/models.rs`: data types
- `packaging/macos/bundle.sh`: builds `Rustcord.app`
- `.github/workflows/build.yml`: builds the macOS app on GitHub Actions

## Discord's rules

Discord's Terms of Service don't allow third-party apps to log in with a personal user token, so Rustcord never asks for one. Rustcord only uses bot tokens and Discord's official bot API.
