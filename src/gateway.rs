//! Discord Gateway (WebSocket) client: heartbeats, identify, dispatch events.

use crate::models::{Channel, Guild, Message, MessageDelete, MessageUpdate, User};
use anyhow::{bail, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";

/// GUILDS | GUILD_MESSAGES | DIRECT_MESSAGES | MESSAGE_CONTENT (privileged; enable it in the dev portal)
const INTENTS: u64 = (1 << 0) | (1 << 9) | (1 << 12) | (1 << 15);

#[derive(Debug)]
pub enum GatewayEvent {
    Status(String),
    Ready { user: User, dms: Vec<Channel> },
    GuildCreate(Guild),
    ChannelCreate(Channel),
    MessageCreate(Message),
    MessageUpdate(MessageUpdate),
    MessageDelete(MessageDelete),
    Error(String),
}

#[derive(Deserialize)]
struct Payload {
    op: u8,
    #[serde(default)]
    d: Value,
    #[serde(default)]
    s: Option<u64>,
    #[serde(default)]
    t: Option<String>,
}

/// Runs forever, reconnecting after failures.
pub async fn run(token: String, out: UnboundedSender<GatewayEvent>, ctx: egui::Context) {
    let mut seq: Option<u64> = None;
    loop {
        let _ = out.send(GatewayEvent::Status("Connecting to gateway…".into()));
        ctx.request_repaint();
        if let Err(e) = session(&token, &out, &ctx, &mut seq).await {
            let text = e.to_string();
            // Discord rejected the token (4004). Retrying would never work, so stop.
            if text.contains("4004") || text.contains("Authentication failed") {
                let _ = out.send(GatewayEvent::Error(
                    "Discord rejected the bot token. Paste a current token to connect.".into(),
                ));
                let _ = out.send(GatewayEvent::Status("Not connected".into()));
                ctx.request_repaint();
                return;
            }
            let _ = out.send(GatewayEvent::Error(format!("Gateway: {e}")));
            ctx.request_repaint();
        }
        let _ = out.send(GatewayEvent::Status("Reconnecting in 5s…".into()));
        ctx.request_repaint();
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn session(
    token: &str,
    out: &UnboundedSender<GatewayEvent>,
    ctx: &egui::Context,
    seq: &mut Option<u64>,
) -> Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(GATEWAY_URL).await?;
    let (mut write, mut read) = ws.split();

    // 1. Hello gives us the heartbeat interval.
    let hello: Payload = match read.next().await {
        Some(Ok(WsMessage::Text(t))) => serde_json::from_str(&t)?,
        other => bail!("unexpected first frame: {other:?}"),
    };
    if hello.op != 10 {
        bail!("expected Hello (op 10), got op {}", hello.op);
    }
    let interval_ms = hello.d["heartbeat_interval"].as_u64().unwrap_or(41_250);

    // 2. Identify. The gateway takes the raw token (no "Bot " prefix).
    let identify = json!({
        "op": 2,
        "d": {
            "token": token,
            "intents": INTENTS,
            "properties": {
                "os": std::env::consts::OS,
                "browser": "rustcord",
                "device": "rustcord"
            }
        }
    });
    write.send(WsMessage::Text(identify.to_string())).await?;
    let _ = out.send(GatewayEvent::Status("Identifying…".into()));

    // 3. Heartbeat loop + event loop.
    let mut beat = tokio::time::interval(Duration::from_millis(interval_ms));
    beat.tick().await; // first tick is immediate; skip it

    loop {
        tokio::select! {
            _ = beat.tick() => {
                write
                    .send(WsMessage::Text(json!({ "op": 1, "d": *seq }).to_string()))
                    .await?;
            }
            frame = read.next() => {
                let frame = match frame {
                    Some(f) => f?,
                    None => bail!("connection closed"),
                };
                let text = match frame {
                    WsMessage::Text(t) => t,
                    WsMessage::Close(c) => bail!("server closed connection: {c:?}"),
                    _ => continue,
                };
                let p: Payload = serde_json::from_str(&text)?;
                if let Some(s) = p.s {
                    *seq = Some(s);
                }
                match p.op {
                    // Dispatch: a real event
                    0 => {
                        let t = p.t.as_deref().unwrap_or_default();
                        match dispatch(t, p.d) {
                            Ok(Some(ev)) => {
                                if matches!(ev, GatewayEvent::Ready { .. }) {
                                    let _ = out.send(GatewayEvent::Status("Connected".into()));
                                }
                                let _ = out.send(ev);
                                ctx.request_repaint();
                            }
                            Ok(None) => {}
                            Err(e) => {
                                let _ = out.send(GatewayEvent::Error(format!("bad {t} event: {e}")));
                            }
                        }
                    }
                    // Server asked for a heartbeat right now
                    1 => {
                        write
                            .send(WsMessage::Text(json!({ "op": 1, "d": *seq }).to_string()))
                            .await?;
                    }
                    // Reconnect / invalid session: drop and reconnect
                    7 | 9 => bail!("server requested reconnect (op {})", p.op),
                    _ => {} // 11 = heartbeat ACK and others
                }
            }
        }
    }
}

fn dispatch(t: &str, d: Value) -> Result<Option<GatewayEvent>> {
    Ok(match t {
        "READY" => {
            let user = serde_json::from_value(d["user"].clone())?;
            // Existing DM channels arrive in READY as "private_channels".
            let dms: Vec<Channel> =
                serde_json::from_value(d["private_channels"].clone()).unwrap_or_default();
            Some(GatewayEvent::Ready {
                user,
                dms: dms.into_iter().filter(|c| c.is_dm()).collect(),
            })
        }
        "GUILD_CREATE" => Some(GatewayEvent::GuildCreate(serde_json::from_value(d)?)),
        "CHANNEL_CREATE" => {
            let ch: Channel = serde_json::from_value(d)?;
            ch.is_dm().then_some(GatewayEvent::ChannelCreate(ch))
        }
        "MESSAGE_CREATE" => Some(GatewayEvent::MessageCreate(serde_json::from_value(d)?)),
        "MESSAGE_UPDATE" => Some(GatewayEvent::MessageUpdate(serde_json::from_value(d)?)),
        "MESSAGE_DELETE" => Some(GatewayEvent::MessageDelete(serde_json::from_value(d)?)),
        _ => None,
    })
}
