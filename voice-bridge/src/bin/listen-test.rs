//! Connects to the bridge's voice output and reports what it receives.
use std::io::Read;
use std::net::TcpStream;

fn main() {
    let addr = std::env::var("RUSTCORD_VOICE_OUT").unwrap_or_else(|_| "127.0.0.1:4011".into());
    let mut stream = TcpStream::connect(&addr).unwrap_or_else(|e| panic!("could not reach {addr}: {e}"));
    println!("listening for voice from {addr}");
    let mut len_buf = [0u8; 4];
    let mut frames = 0u64;
    let mut speakers = std::collections::BTreeMap::<u32, u64>::new();
    loop {
        if stream.read_exact(&mut len_buf).is_err() {
            break;
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut frame = vec![0u8; len];
        if stream.read_exact(&mut frame).is_err() {
            break;
        }
        let ssrc = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
        *speakers.entry(ssrc).or_insert(0) += 1;
        frames += 1;
        if frames % 50 == 0 {
            println!("frames: {frames}, per speaker: {speakers:?}");
        }
    }
}
