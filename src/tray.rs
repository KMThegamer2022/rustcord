//! Menu bar (macOS) / system tray (Windows) icon, so Rustcord keeps running after its window is closed.

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum TrayAction {
    Show,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    show_id: MenuId,
    quit_id: MenuId,
}

impl Tray {
    /// Creates the tray icon. Must be called on the main thread after the event loop exists.
    pub fn new() -> Option<Tray> {
        let show = MenuItem::new("Show Rustcord", true, None);
        let quit = MenuItem::new("Quit Rustcord", true, None);
        let menu = Menu::new();
        menu.append(&show).ok()?;
        menu.append(&quit).ok()?;

        let png = image::load_from_memory(include_bytes!("../assets/icon.png")).ok()?;
        let small = png.resize(64, 64, image::imageops::FilterType::Lanczos3).to_rgba8();
        let (w, h) = small.dimensions();
        let icon = tray_icon::Icon::from_rgba(small.into_raw(), w, h).ok()?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Rustcord")
            .with_icon(icon)
            .build()
            .ok()?;

        Some(Tray {
            _icon: tray,
            show_id: show.id().clone(),
            quit_id: quit.id().clone(),
        })
    }

    /// Returns what the user asked for from the tray since the last call.
    pub fn poll(&self) -> Vec<TrayAction> {
        let mut actions = Vec::new();
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == self.show_id {
                actions.push(TrayAction::Show);
            } else if ev.id == self.quit_id {
                actions.push(TrayAction::Quit);
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                actions.push(TrayAction::Show);
            }
        }
        actions
    }
}
