//! Rustcord - a native Discord client written in Rust.

mod app;
mod gateway;
mod models;
mod net;
mod relay;
mod tray;
mod update;
mod voice;
mod rest;

fn main() -> eframe::Result<()> {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (evt_tx, evt_rx) = tokio::sync::mpsc::unbounded_channel();

    // Window/taskbar icon: same artwork as the bot's profile picture.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Rustcord")
        .with_inner_size([1200.0, 760.0]);
    if let Ok(img) = image::load_from_memory(include_bytes!("../assets/icon.png")) {
        let rgba = img.to_rgba8();
        viewport = viewport.with_icon(egui::IconData {
            width: rgba.width(),
            height: rgba.height(),
            rgba: rgba.into_raw(),
        });
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Rustcord",
        options,
        Box::new(move |cc| {
            // Networking runs on its own tokio runtime so the UI thread never blocks.
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("failed to start tokio runtime")
                    .block_on(net::run(cmd_rx, evt_tx, ctx));
            });
            Ok(Box::new(app::App::new(cmd_tx, evt_rx)))
        }),
    )
}
