//! Bridge between the egui UI thread and the async network tasks.

use crate::gateway::{self, GatewayEvent};
use crate::models::Message;
use crate::rest::Rest;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

pub enum NetCommand {
    Connect(String),
    FetchMessages(String),
    Send { channel_id: String, content: String },
    Edit { channel_id: String, message_id: String, content: String },
    Delete { channel_id: String, message_id: String },
    FetchImage(String),
}

pub enum NetEvent {
    Gateway(GatewayEvent),
    Messages { channel_id: String, messages: Vec<Message> },
    Image { url: String, bytes: Vec<u8> },
    Error(String),
}

/// Runs a REST call in the background and reports failures to the UI.
fn spawn_rest<F, Fut>(
    evts: &UnboundedSender<NetEvent>,
    ctx: &egui::Context,
    what: &'static str,
    fut: F,
) where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send,
{
    let (evts, ctx) = (evts.clone(), ctx.clone());
    tokio::spawn(async move {
        if let Err(e) = fut().await {
            let _ = evts.send(NetEvent::Error(format!("{what} failed: {e}")));
            ctx.request_repaint();
        }
    });
}

pub async fn run(
    mut cmds: UnboundedReceiver<NetCommand>,
    evts: UnboundedSender<NetEvent>,
    ctx: egui::Context,
) {
    let (gw_tx, mut gw_rx) = unbounded_channel::<GatewayEvent>();
    let mut rest: Option<Rest> = None;
    let mut gateway_task: Option<JoinHandle<()>> = None;
    let http = reqwest::Client::builder()
        .user_agent("Rustcord/0.1 (native client)")
        .build()
        .expect("failed to build image client");

    loop {
        tokio::select! {
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    NetCommand::Connect(token) => {
                        if let Some(h) = gateway_task.take() {
                            h.abort();
                        }
                        rest = Some(Rest::new(&token));
                        gateway_task = Some(tokio::spawn(gateway::run(token, gw_tx.clone(), ctx.clone())));
                    }
                    NetCommand::FetchMessages(channel_id) => {
                        let Some(r) = rest.clone() else { continue };
                        let (evts2, ctx2) = (evts.clone(), ctx.clone());
                        tokio::spawn(async move {
                            let ev = match r.messages(&channel_id, 50).await {
                                Ok(messages) => NetEvent::Messages { channel_id, messages },
                                Err(e) => NetEvent::Error(format!("Could not load messages: {e}")),
                            };
                            let _ = evts2.send(ev);
                            ctx2.request_repaint();
                        });
                    }
                    NetCommand::Send { channel_id, content } => {
                        let Some(r) = rest.clone() else { continue };
                        spawn_rest(&evts, &ctx, "Send", move || async move {
                            r.send(&channel_id, &content).await.map(|_| ())
                        });
                    }
                    NetCommand::Edit { channel_id, message_id, content } => {
                        let Some(r) = rest.clone() else { continue };
                        spawn_rest(&evts, &ctx, "Edit", move || async move {
                            r.edit(&channel_id, &message_id, &content).await.map(|_| ())
                        });
                    }
                    NetCommand::FetchImage(url) => {
                        let (http, evts, ctx) = (http.clone(), evts.clone(), ctx.clone());
                        tokio::spawn(async move {
                            let res = async {
                                let r = http.get(&url).send().await?.error_for_status()?;
                                r.bytes().await
                            }
                            .await;
                            if let Ok(bytes) = res {
                                let _ = evts.send(NetEvent::Image { url, bytes: bytes.to_vec() });
                                ctx.request_repaint();
                            }
                        });
                    }
                    NetCommand::Delete { channel_id, message_id } => {
                        let Some(r) = rest.clone() else { continue };
                        spawn_rest(&evts, &ctx, "Delete", move || async move {
                            r.delete(&channel_id, &message_id).await
                        });
                    }
                }
            }
            Some(ev) = gw_rx.recv() => {
                let _ = evts.send(NetEvent::Gateway(ev));
                ctx.request_repaint();
            }
        }
    }
}
