//! Rustcord voice bridge.
//!
//! The Discord bot joins a voice channel. Rustcord apps send their microphone audio to a
//! local socket (see `mic-sender`), and the bridge plays it into the channel.
//!
//! Environment:
//!   RUSTCORD_BOT_TOKEN       bot token (never printed or stored)
//!   RUSTCORD_VOICE_GUILD     server ID
//!   RUSTCORD_VOICE_CHANNEL   voice channel ID
//!   RUSTCORD_VOICE_LISTEN    where to accept audio (default 127.0.0.1:4010)
//!
//! Audio socket protocol: each frame is a u32 little-endian byte count followed by that many
//! bytes of interleaved stereo 32-bit float PCM at 48 kHz (8 bytes per sample frame).

use serenity::all::{ChannelId, GatewayIntents, GuildId, Ready};
use serenity::async_trait;
use serenity::prelude::*;
use songbird::input::{Input, RawAdapter};
use songbird::{CoreEvent, Event, EventContext, EventHandler as VoiceEventHandler, SerenityInit};
use std::io::{Read, Seek, SeekFrom};
use std::net::TcpListener;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use symphonia_core::io::MediaSource;

/// Audio from the socket goes here; the voice track reads from it.
static AUDIO_TX: OnceLock<Sender<Vec<u8>>> = OnceLock::new();
static AUDIO_RX: Mutex<Option<Receiver<Vec<u8>>>> = Mutex::new(None);
static MANAGER: OnceLock<std::sync::Arc<songbird::Songbird>> = OnceLock::new();
/// Apps that asked to receive other people's voice (see RUSTCORD_VOICE_OUT).
static LISTENERS: Mutex<Vec<std::net::TcpStream>> = Mutex::new(Vec::new());

/// Sends one speaker's decoded audio to every listener.
///
/// Frame format: u32 LE byte count, then u32 LE speaker ID (SSRC), then interleaved stereo
/// 16-bit PCM at 48 kHz.
fn broadcast(ssrc: u32, pcm: &[i16]) {
    let mut frame = Vec::with_capacity(4 + pcm.len() * 2);
    frame.extend_from_slice(&ssrc.to_le_bytes());
    for sample in pcm {
        frame.extend_from_slice(&sample.to_le_bytes());
    }
    let mut listeners = LISTENERS.lock().unwrap();
    let len = (frame.len() as u32).to_le_bytes();
    listeners.retain_mut(|stream| {
        use std::io::Write;
        stream.write_all(&len).and_then(|_| stream.write_all(&frame)).is_ok()
    });
}

/// Accepts apps that want other people's voice.
fn serve_listeners(addr: String) {
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("could not listen for voice on {addr}: {e}");
            return;
        }
    };
    println!("sending voice to listeners on {addr}");
    for stream in listener.incoming().flatten() {
        let _ = stream.set_write_timeout(Some(Duration::from_millis(50)));
        let _ = stream.set_nodelay(true);
        LISTENERS.lock().unwrap().push(stream);
    }
}

/// Read side of the audio channel. Plays silence when no audio is arriving, so the track
/// never stalls.
struct ChannelSource {
    rx: Mutex<Receiver<Vec<u8>>>,
    pending: Vec<u8>,
}

impl Read for ChannelSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending.is_empty() {
            let rx = self.rx.lock().unwrap();
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(data) => self.pending = data,
                Err(RecvTimeoutError::Timeout) => {
                    buf.fill(0);
                    return Ok(buf.len());
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

impl Seek for ChannelSource {
    fn seek(&mut self, _pos: SeekFrom) -> std::io::Result<u64> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "live audio cannot seek"))
    }
}

impl MediaSource for ChannelSource {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// Symphonia needs the raw format header; RawAdapter adds it for us.
fn live_input(rx: Receiver<Vec<u8>>) -> Input {
    let source = ChannelSource { rx: Mutex::new(rx), pending: Vec::new() };
    RawAdapter::new(source, 48_000, 2).into()
}

/// Accepts audio connections and forwards each frame to the voice track.
fn serve_audio(addr: String) {
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("could not listen for audio on {addr}: {e}");
            return;
        }
    };
    println!("accepting microphone audio on {addr}");
    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || {
            let mut stream = stream;
            let mut len_buf = [0u8; 4];
            loop {
                if stream.read_exact(&mut len_buf).is_err() {
                    break;
                }
                let len = u32::from_le_bytes(len_buf) as usize;
                if len == 0 || len > 1_000_000 {
                    break;
                }
                let mut frame = vec![0u8; len];
                if stream.read_exact(&mut frame).is_err() {
                    break;
                }
                if let Some(tx) = AUDIO_TX.get() {
                    let _ = tx.send(frame);
                }
            }
        });
    }
}

/// Below this loudness (RMS of 16-bit samples) a frame counts as silence.
const SILENCE_THRESHOLD: f64 = 40.0;

/// Receives decoded voice from people in the channel (every 20 ms) and reports how loud each
/// speaker is. Audio is forwarded to Rustcord apps in a later step.
struct VoiceReceiver;

#[async_trait]
impl VoiceEventHandler for VoiceReceiver {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        if let EventContext::VoiceTick(tick) = ctx {
                        for (ssrc, data) in &tick.speaking {
                if let Some(pcm) = &data.decoded_voice {
                    let sum: f64 = pcm.iter().map(|s| (*s as f64) * (*s as f64)).sum();
                    let rms = (sum / pcm.len().max(1) as f64).sqrt();
                    // Skip silence so listeners only get audio when someone is talking.
                    if rms > SILENCE_THRESHOLD {
                        broadcast(*ssrc, pcm);
                    }
                }
            }
        }
        None
    }
}

struct Handler {
    guild: GuildId,
    channel: ChannelId,
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        println!("connected as {}", ready.user.name);
        let manager = songbird::get(&ctx).await.expect("songbird is registered");
        let _ = MANAGER.set(manager.clone());
        match manager.join(self.guild, self.channel).await {
            Ok(call) => {
                println!("joined the voice channel; waiting for microphone audio");
                let rx = AUDIO_RX.lock().unwrap().take();
                if let Some(rx) = rx {
                    let mut call = call.lock().await;
                    call.play_input(live_input(rx));
                    call.add_global_event(Event::Core(CoreEvent::VoiceTick), VoiceReceiver);
                }
            }
            Err(e) => eprintln!("could not join the voice channel: {e:?}"),
        }
    }
}

fn env_id(name: &str) -> u64 {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("set {name}"))
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("{name} must be a number"))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,songbird=warn".into()),
        )
        .init();

    let (tx, rx) = channel();
    let _ = AUDIO_TX.set(tx);
    *AUDIO_RX.lock().unwrap() = Some(rx);
    let listen = std::env::var("RUSTCORD_VOICE_LISTEN").unwrap_or_else(|_| "127.0.0.1:4010".into());
    std::thread::spawn(move || serve_audio(listen));
    let out = std::env::var("RUSTCORD_VOICE_OUT").unwrap_or_else(|_| "127.0.0.1:4011".into());
    std::thread::spawn(move || serve_listeners(out));

    let guild = GuildId::new(env_id("RUSTCORD_VOICE_GUILD"));
    let token = std::env::var("RUSTCORD_BOT_TOKEN").expect("set RUSTCORD_BOT_TOKEN");
    let handler = Handler {
        guild,
        channel: ChannelId::new(env_id("RUSTCORD_VOICE_CHANNEL")),
    };
    let intents = GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES;
    let mut client = Client::builder(token, intents)
        .event_handler(handler)
        .register_songbird_from_config(songbird::Config::default().decode_mode(songbird::driver::DecodeMode::Decode(Default::default())))
        .await
        .expect("could not create the Discord client");

    tokio::select! {
        r = client.start() => {
            if let Err(e) = r { eprintln!("client error: {e:?}"); }
        }
        _ = tokio::signal::ctrl_c() => {
            println!("stopping: leaving the voice channel");
            if let Some(manager) = MANAGER.get() {
                let _ = manager.leave(guild).await;
            }
        }
    }
}
