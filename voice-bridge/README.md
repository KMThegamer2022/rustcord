# Rustcord voice bridge

A Discord bot that joins a voice channel and carries voice between Rustcord apps and that channel.

- `rustcord-voice`: the bridge. Set `RUSTCORD_BOT_TOKEN`, `RUSTCORD_VOICE_GUILD` and `RUSTCORD_VOICE_CHANNEL`, then run it. It listens for microphone audio on `127.0.0.1:4010` and sends other people's voice to listeners on `127.0.0.1:4011`.
- `mic-sender`: a command-line microphone sender for testing.
- `listen-test`: a command-line listener for testing.

Voice needs the bot to be in the server with **Connect** and **Speak** permission in the voice channel. It runs on your own computer for now.

Built against songbird's `current` branch, which includes the DAVE voice encryption used by Discord.
