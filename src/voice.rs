//! Voice chat through the Rustcord voice bridge.
//!
//! The bridge (a separate program) is in the Discord voice channel. This module:
//! - captures this computer's microphone and sends it to the bridge, and
//! - receives everyone else's voice from the bridge and plays it, mixed together.
//!
//! Both directions use 48 kHz stereo. Mute stops sending; deafen silences playback.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

pub const SEND_ADDR: &str = "127.0.0.1:4010";
pub const RECEIVE_ADDR: &str = "127.0.0.1:4011";

/// Keep at most half a second of audio per speaker so latency can't grow forever.
const MAX_BUFFERED_SAMPLES: usize = 48_000;

/// A running voice connection. Dropping it stops the microphone and playback.
pub struct VoiceSession {
    _mic: Option<cpal::Stream>,
    _speakers: cpal::Stream,
    pub muted: Arc<AtomicBool>,
    pub deafened: Arc<AtomicBool>,
    pub status: Arc<Mutex<String>>,
}

type Speakers = Arc<Mutex<HashMap<u32, VecDeque<f32>>>>;

impl VoiceSession {
    /// Starts the microphone, the speaker output and the connections to the bridge.
    pub fn start() -> Result<VoiceSession, String> {
        let muted = Arc::new(AtomicBool::new(false));
        let deafened = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new("Connecting to voice…".to_string()));

        // Speakers: mix everyone's voice into one output.
        let host = cpal::default_host();
        let out_device = host.default_output_device().ok_or("no speaker found")?;
        let out_config = out_device.default_output_config().map_err(|e| format!("speaker: {e}"))?;
        if out_config.sample_rate().0 != 48_000 {
            return Err(format!("speaker runs at {} Hz; 48000 Hz is required", out_config.sample_rate().0));
        }
        let out_channels = out_config.channels() as usize;
        let speakers: Speakers = Arc::new(Mutex::new(HashMap::new()));
        let out_stream_config: cpal::StreamConfig = out_config.clone().into();
        let speakers_for_output = speakers.clone();
        let deafened_for_output = deafened.clone();
        let speaker_stream = out_device
            .build_output_stream(
                &out_stream_config,
                move |out: &mut [f32], _| {
                    let deaf = deafened_for_output.load(Ordering::Relaxed);
                    let mut mix = speakers_for_output.lock().unwrap();
                    for frame in out.chunks_mut(out_channels) {
                        let (mut left, mut right) = (0.0f32, 0.0f32);
                        if !deaf {
                            for queue in mix.values_mut() {
                                if let (Some(l), Some(r)) = (queue.pop_front(), queue.pop_front()) {
                                    left += l;
                                    right += r;
                                }
                            }
                        }
                        frame[0] = left.clamp(-1.0, 1.0);
                        if out_channels > 1 {
                            frame[1] = right.clamp(-1.0, 1.0);
                        }
                        for extra in frame.iter_mut().skip(2) {
                            *extra = 0.0;
                        }
                    }
                    mix.retain(|_, q| !q.is_empty());
                },
                |e| eprintln!("speaker error: {e}"),
                None,
            )
            .map_err(|e| format!("could not open the speaker: {e}"))?;
        speaker_stream.play().map_err(|e| format!("could not start the speaker: {e}"))?;

        // Receive: read frames from the bridge and add them to each speaker's queue.
        let speakers_for_receive = speakers.clone();
        let status_for_receive = status.clone();
        std::thread::spawn(move || receive_loop(speakers_for_receive, status_for_receive));

        // Microphone: capture and send to the bridge (skipped while muted).
        let (mic_tx, mic_rx) = channel::<Vec<u8>>();
        let mic_stream = start_microphone(mic_tx, muted.clone(), status.clone());
        std::thread::spawn(move || send_loop(mic_rx));

        Ok(VoiceSession {
            _mic: mic_stream,
            _speakers: speaker_stream,
            muted,
            deafened,
            status,
        })
    }
}

fn receive_loop(speakers: Speakers, status: Arc<Mutex<String>>) {
    let mut stream = match TcpStream::connect(RECEIVE_ADDR) {
        Ok(s) => s,
        Err(e) => {
            *status.lock().unwrap() = format!("Voice bridge not running ({e})");
            return;
        }
    };
    *status.lock().unwrap() = "Connected to voice".into();
    let mut len_buf = [0u8; 4];
    loop {
        if stream.read_exact(&mut len_buf).is_err() {
            break;
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if !(4..=1_000_000).contains(&len) {
            break;
        }
        let mut frame = vec![0u8; len];
        if stream.read_exact(&mut frame).is_err() {
            break;
        }
        let ssrc = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
        let mut mix = speakers.lock().unwrap();
        let queue = mix.entry(ssrc).or_default();
        for pair in frame[4..].chunks_exact(2) {
            let sample = i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32_768.0;
            queue.push_back(sample);
        }
        while queue.len() > MAX_BUFFERED_SAMPLES {
            queue.pop_front();
        }
    }
    *status.lock().unwrap() = "Voice bridge disconnected".into();
}

fn start_microphone(tx: Sender<Vec<u8>>, muted: Arc<AtomicBool>, status: Arc<Mutex<String>>) -> Option<cpal::Stream> {
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => {
            *status.lock().unwrap() = "No microphone found".into();
            return None;
        }
    };
    let config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            *status.lock().unwrap() = format!("Microphone: {e}");
            return None;
        }
    };
    if config.sample_rate().0 != 48_000 {
        *status.lock().unwrap() = format!("Microphone runs at {} Hz; 48000 Hz is required", config.sample_rate().0);
        return None;
    }
    let channels = config.channels() as usize;
    let stream_config: cpal::StreamConfig = config.clone().into();
    let err_status = status.clone();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &stream_config,
            move |data: &[f32], _| {
                if !muted.load(Ordering::Relaxed) {
                    let _ = tx.send(to_stereo_bytes(data, channels));
                }
            },
            move |e| *err_status.lock().unwrap() = format!("Microphone error: {e}"),
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &stream_config,
            move |data: &[i16], _| {
                if !muted.load(Ordering::Relaxed) {
                    let floats: Vec<f32> = data.iter().map(|s| *s as f32 / 32_768.0).collect();
                    let _ = tx.send(to_stereo_bytes(&floats, channels));
                }
            },
            move |e| *err_status.lock().unwrap() = format!("Microphone error: {e}"),
            None,
        ),
        other => {
            *status.lock().unwrap() = format!("Unsupported microphone format: {other:?}");
            return None;
        }
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            *status.lock().unwrap() = format!("Could not open the microphone: {e}");
            return None;
        }
    };
    if let Err(e) = stream.play() {
        *status.lock().unwrap() = format!("Could not start the microphone: {e}");
        return None;
    }
    Some(stream)
}

/// Interleaved mono/stereo float samples -> stereo 32-bit float bytes.
fn to_stereo_bytes(data: &[f32], channels: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / channels.max(1) * 8);
    for frame in data.chunks(channels) {
        let left = frame[0];
        let right = if channels > 1 { frame[1] } else { frame[0] };
        out.extend_from_slice(&left.to_le_bytes());
        out.extend_from_slice(&right.to_le_bytes());
    }
    out
}

fn send_loop(rx: std::sync::mpsc::Receiver<Vec<u8>>) {
    let mut stream = match TcpStream::connect(SEND_ADDR) {
        Ok(s) => s,
        Err(_) => return,
    };
    for frame in rx {
        let len = (frame.len() as u32).to_le_bytes();
        if stream.write_all(&len).and_then(|_| stream.write_all(&frame)).is_err() {
            break;
        }
    }
}
