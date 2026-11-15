//! Sends this computer's microphone to the voice bridge.
//!
//! Captures the default input device, converts it to stereo 32-bit float at 48 kHz, and
//! sends it to the bridge's audio socket (RUSTCORD_VOICE_LISTEN, default 127.0.0.1:4010).
//! The frame format matches `rustcord-voice`.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::io::Write;
use std::net::TcpStream;
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

fn send_stereo(data: &[f32], channels: usize, tx: &Sender<Vec<u8>>) {
    let mut out = Vec::with_capacity(data.len() / channels * 8);
    for frame in data.chunks(channels) {
        let left = frame[0];
        let right = if channels > 1 { frame[1] } else { frame[0] };
        out.extend_from_slice(&left.to_le_bytes());
        out.extend_from_slice(&right.to_le_bytes());
    }
    let _ = tx.send(out);
}

fn main() {
    let addr = std::env::var("RUSTCORD_VOICE_LISTEN").unwrap_or_else(|_| "127.0.0.1:4010".into());

    let host = cpal::default_host();
    let device = host.default_input_device().expect("no microphone found");
    let config = device.default_input_config().expect("could not read the microphone settings");
    let channels = config.channels() as usize;
    println!(
        "microphone: {} ({} Hz, {} channels, {:?})",
        device.name().unwrap_or_default(),
        config.sample_rate().0,
        channels,
        config.sample_format()
    );
    if config.sample_rate().0 != 48_000 {
        panic!("this microphone runs at {} Hz; only 48000 Hz is supported for now", config.sample_rate().0);
    }

    let (tx, rx) = channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut stream = TcpStream::connect(&addr).unwrap_or_else(|e| panic!("could not reach the bridge at {addr}: {e}"));
        println!("connected to the bridge at {addr}");
        for frame in rx {
            let len = (frame.len() as u32).to_le_bytes();
            if stream.write_all(&len).and_then(|_| stream.write_all(&frame)).is_err() {
                eprintln!("lost the connection to the bridge");
                break;
            }
        }
    });

    let stream_config: cpal::StreamConfig = config.clone().into();
    let err_fn = |e| eprintln!("microphone error: {e}");
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let tx = tx.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[f32], _| send_stereo(data, channels, &tx),
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let tx = tx.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[i16], _| {
                    let floats: Vec<f32> = data.iter().map(|s| *s as f32 / 32_768.0).collect();
                    send_stereo(&floats, channels, &tx);
                },
                err_fn,
                None,
            )
        }
        other => panic!("unsupported microphone sample format: {other:?}"),
    }
    .expect("could not open the microphone");
    stream.play().expect("could not start the microphone");

    println!("sending your microphone to the bridge; press Ctrl+C to stop");
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}
