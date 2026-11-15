//! egui user interface: login, DMs, servers, channels, chat, editing, composer.
//! Styled to feel familiar to Discord users (dark theme, server rail, avatars).
//!
//! Two ways in:
//! - **Rustcord account** (default): sign in to the relay server. Servers, channels
//!   and messages go through the relay, which posts as you via webhooks.
//! - **Discord bot token** (advanced): connect straight to Discord as the bot.

use crate::gateway::GatewayEvent;
use crate::models::{Channel, Guild, Message, User};
use crate::net::{NetCommand, NetEvent};
use crate::relay::{self, Profile, RelayEvent};
use crate::update;
use eframe::egui::{
    self, Align2, Color32, FontId, Frame, Margin, Rect, RichText, Sense, TextureHandle, Vec2,
};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

const MAX_MESSAGES_PER_CHANNEL: usize = 300;
const KEYCHAIN_SERVICE: &str = "rustcord";
const KEYCHAIN_ACCOUNT: &str = "bot-token";
const RELAY_ACCOUNT: &str = "relay-session";
const DEFAULT_RELAY: &str = "https://kmtg.org/rustcord";
const POLL_EVERY: Duration = Duration::from_secs(4);

fn keychain_entry() -> Option<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).ok()
}

/// Returns the saved bot token from the macOS Keychain, if there is one.
fn load_token() -> Option<String> {
    keychain_entry()?.get_password().ok().filter(|t| !t.is_empty())
}

fn save_token(token: &str) -> Result<(), String> {
    let entry = keychain_entry().ok_or("could not open Keychain")?;
    entry.set_password(token).map_err(|e| format!("could not save token: {e}"))
}

fn forget_token() {
    if let Some(entry) = keychain_entry() {
        let _ = entry.delete_credential();
    }
}

fn relay_entry() -> Option<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, RELAY_ACCOUNT).ok()
}

/// Saved relay session as (server address, session token).
fn load_relay_session() -> Option<(String, String)> {
    let raw = relay_entry()?.get_password().ok()?;
    let (url, token) = raw.split_once('\n')?;
    Some((url.to_string(), token.to_string()))
}

fn save_relay_session(url: &str, token: &str) -> Result<(), String> {
    let entry = relay_entry().ok_or("could not open Keychain")?;
    entry
        .set_password(&format!("{url}\n{token}"))
        .map_err(|e| format!("could not save sign-in: {e}"))
}

fn forget_relay_session() {
    if let Some(entry) = relay_entry() {
        let _ = entry.delete_credential();
    }
}

// Dark palette in the same spirit as Discord's default theme.
const SERVER_BG: Color32 = Color32::from_rgb(30, 31, 34);
const CHANNEL_BG: Color32 = Color32::from_rgb(43, 45, 49);
const CHAT_BG: Color32 = Color32::from_rgb(49, 51, 56);
const INPUT_BG: Color32 = Color32::from_rgb(56, 58, 64);
const TEXT: Color32 = Color32::from_rgb(219, 222, 225);
const MUTED: Color32 = Color32::from_rgb(148, 155, 164);
const ACCENT: Color32 = Color32::from_rgb(88, 101, 242);
const HOVER: Color32 = Color32::from_rgb(53, 55, 60);
const SELECTED: Color32 = Color32::from_rgb(64, 66, 73);

#[derive(Clone, PartialEq)]
enum View {
    Home,
    Guild(String),
}

struct Editing {
    channel_id: String,
    message_id: String,
    text: String,
}

pub struct App {
    cmd_tx: UnboundedSender<NetCommand>,
    evt_rx: UnboundedReceiver<NetEvent>,

    // Relay (account) connection
    relay_tx: Sender<RelayEvent>,
    relay_rx: Receiver<RelayEvent>,
    ctx: Option<egui::Context>,
    use_relay: bool,
    relay_url: String,
    relay_user: String,
    relay_pass: String,
    relay_session: Option<(String, String)>,
    relay_busy: bool,
    stay_signed_in: bool,
    pending_restore: Option<(String, String)>,
    profile: Option<Profile>,
    profile_open: bool,
    edit_name: String,
    edit_avatar: String,
    edit_desc: String,
    add_server_open: bool,
    add_server_input: String,
    last_poll: Option<Instant>,

    // Background running: the tray icon keeps Rustcord available after the window closes
    tray: Option<crate::tray::Tray>,
    quitting: bool,

    // Voice: connected voice session and the channel it is for
    voice: Option<crate::voice::VoiceSession>,
    voice_channel: Option<String>,

    // Update check
    update_rx: Receiver<update::UpdateInfo>,
    update_checked: bool,
    update_available: Option<update::UpdateInfo>,
    update_msg_tx: Sender<String>,
    update_msg_rx: Receiver<String>,
    update_msg: Option<String>,
    update_busy: bool,

    // Discord bot connection
    token_input: String,
    logged_in: bool,
    connected: bool,
    status: String,
    last_error: Option<String>,

    me: Option<User>,
    guilds: Vec<Guild>,
    dms: Vec<Channel>,
    view: View,
    selected_channel: Option<String>,
    messages: HashMap<String, Vec<Message>>,
    composer: String,
    editing: Option<Editing>,

    textures: HashMap<String, TextureHandle>,
    requested: HashSet<String>,
    styled: bool,

    remember: bool,
    has_saved_token: bool,
}

/// One line of the channel sidebar.
enum Row<'a> {
    Header(&'a str),
    Chan(&'a Channel),
}

impl App {
    pub fn new(cmd_tx: UnboundedSender<NetCommand>, evt_rx: UnboundedReceiver<NetEvent>) -> Self {
        let (relay_tx, relay_rx) = channel();

        // A saved Rustcord account session is restored automatically. A saved bot
        // token is kept for the "Discord bot token" option but never connects on its own,
        // since an old or reset token would fail in the background.
        let saved_relay = load_relay_session();
        let has_saved_token = load_token().is_some();
        let mut status = "Not connected".to_string();
        let logged_in = false;
        let mut relay_busy = false;
        let mut pending_restore = None;
        if let Some((url, token)) = saved_relay {
            pending_restore = Some((url, token));
            relay_busy = true;
            status = "Signing you back in…".into();
        }

        Self {
            cmd_tx,
            evt_rx,
            relay_tx,
            relay_rx,
            ctx: None,
            use_relay: true,
            relay_url: DEFAULT_RELAY.into(),
            relay_user: String::new(),
            relay_pass: String::new(),
            relay_session: None,
            relay_busy,
            stay_signed_in: true,
            pending_restore,
            profile: None,
            profile_open: false,
            edit_name: String::new(),
            edit_avatar: String::new(),
            edit_desc: String::new(),
            add_server_open: false,
            add_server_input: String::new(),
            last_poll: None,
            update_rx: channel().1,
            tray: crate::tray::Tray::new(),
            quitting: false,
            voice: None,
            voice_channel: None,
            update_checked: false,
            update_available: None,
            update_msg_tx: channel().0,
            update_msg_rx: channel().1,
            update_msg: None,
            update_busy: false,
            token_input: String::new(),
            logged_in,
            connected: false,
            status,
            last_error: None,
            remember: true,
            has_saved_token,
            me: None,
            guilds: Vec::new(),
            dms: Vec::new(),
            view: View::Home,
            selected_channel: None,
            messages: HashMap::new(),
            composer: String::new(),
            editing: None,
            textures: HashMap::new(),
            requested: HashSet::new(),
            styled: false,
        }
    }

    // ---------- Relay requests ----------

    /// Runs a relay call in the background; its result arrives as a RelayEvent.
    fn spawn_relay<F>(&self, f: F)
    where
        F: FnOnce() -> RelayEvent + Send + 'static,
    {
        if let Some(ctx) = self.ctx.clone() {
            relay::spawn(self.relay_tx.clone(), ctx, f);
        }
    }

    fn request_guilds(&self) {
        if let Some((url, token)) = self.relay_session.clone() {
            self.spawn_relay(move || relay::guilds(url, token));
        }
    }

    fn request_channels(&self, guild_id: String) {
        if let Some((url, token)) = self.relay_session.clone() {
            self.spawn_relay(move || relay::channels(url, token, guild_id));
        }
    }

    fn request_messages(&self, guild_id: &str, channel_id: &str) {
        if let Some((url, token)) = self.relay_session.clone() {
            let (g, c) = (guild_id.to_string(), channel_id.to_string());
            self.spawn_relay(move || relay::messages(url, token, g, c));
        }
    }

    /// Signs out of the relay and clears what was shown.
    fn sign_out(&mut self) {
        self.relay_session = None;
        forget_relay_session();
        self.connected = false;
        self.logged_in = false;
        self.relay_busy = false;
        self.me = None;
        self.profile = None;
        self.profile_open = false;
        self.add_server_open = false;
        self.guilds.clear();
        self.messages.clear();
        self.selected_channel = None;
        self.editing = None;
        self.view = View::Home;
        self.status = "Signed out".into();
    }

    fn handle_relay(&mut self, ev: RelayEvent) {
        self.relay_busy = false;
        match ev {
            RelayEvent::LoggedIn { url, token, profile } => {
                if self.stay_signed_in {
                    if let Err(e) = save_relay_session(&url, &token) {
                        self.last_error = Some(e);
                    }
                }
                self.relay_session = Some((url, token));
                self.me = Some(profile.to_user());
                self.status = format!("Signed in as {}", profile.display_name);
                self.profile = Some(profile);
                self.connected = true;
                self.logged_in = true;
                self.last_error = None;
                self.request_guilds();
            }
            RelayEvent::Profile(p) => {
                self.me = Some(p.to_user());
                self.profile = Some(p);
                self.profile_open = false;
                self.status = "Profile saved".into();
                self.last_error = None;
            }
            RelayEvent::Guilds(list) => {
                // Keep channels we already loaded for each server.
                let old = std::mem::take(&mut self.guilds);
                self.guilds = list
                    .into_iter()
                    .map(|mut g| {
                        if let Some(o) = old.iter().find(|o| o.id == g.id) {
                            g.channels = o.channels.clone();
                        }
                        g
                    })
                    .collect();
            }
            RelayEvent::Joined(g) => {
                let id = g.id.clone();
                self.status = format!("Joined {}", g.name);
                match self.guilds.iter_mut().find(|x| x.id == g.id) {
                    Some(existing) => *existing = g,
                    None => self.guilds.push(g),
                }
                self.add_server_open = false;
                self.add_server_input.clear();
                self.last_error = None;
                self.selected_channel = None;
                self.view = View::Guild(id.clone());
                self.request_channels(id);
            }
            RelayEvent::Channels { guild_id, channels } => {
                if let Some(g) = self.guilds.iter_mut().find(|g| g.id == guild_id) {
                    g.channels = channels;
                }
                if self.selected_channel.is_none() && self.view == View::Guild(guild_id.clone()) {
                    let first = self
                        .guilds
                        .iter()
                        .find(|g| g.id == guild_id)
                        .and_then(first_text_channel);
                    if let Some(cid) = first {
                        self.open_channel(cid);
                    }
                }
            }
            RelayEvent::Messages { channel_id, messages } => {
                self.messages.insert(channel_id, messages);
            }
            RelayEvent::Sent { guild_id, channel_id } => {
                self.request_messages(&guild_id, &channel_id);
            }
            RelayEvent::SessionExpired => {
                self.sign_out();
                self.last_error = Some("Your sign-in has ended. Please log in again.".into());
            }
            RelayEvent::Error(e) => self.last_error = Some(e),
        }
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.relay_rx.try_recv() {
            self.handle_relay(ev);
        }
        while let Ok(ev) = self.evt_rx.try_recv() {
            match ev {
                NetEvent::Gateway(GatewayEvent::Status(s)) => self.status = s,
                NetEvent::Gateway(GatewayEvent::Ready { user, dms }) => {
                    self.status = format!("Connected as {}", user.display_name());
                    self.connected = true;
                    self.me = Some(user);
                    for dm in dms {
                        self.add_dm(dm);
                    }
                }
                NetEvent::Gateway(GatewayEvent::ChannelCreate(ch)) => self.add_dm(ch),
                NetEvent::Gateway(GatewayEvent::GuildCreate(guild)) => {
                    match self.guilds.iter_mut().find(|g| g.id == guild.id) {
                        Some(existing) => *existing = guild,
                        None => self.guilds.push(guild),
                    }
                }
                NetEvent::Gateway(GatewayEvent::MessageCreate(msg)) => {
                    if let Some(list) = self.messages.get_mut(&msg.channel_id) {
                        if !list.iter().any(|m| m.id == msg.id) {
                            list.push(msg);
                            if list.len() > MAX_MESSAGES_PER_CHANNEL {
                                let excess = list.len() - MAX_MESSAGES_PER_CHANNEL;
                                list.drain(..excess);
                            }
                        }
                    }
                }
                NetEvent::Gateway(GatewayEvent::MessageUpdate(upd)) => {
                    if let Some(m) = self
                        .messages
                        .get_mut(&upd.channel_id)
                        .and_then(|list| list.iter_mut().find(|m| m.id == upd.id))
                    {
                        if let Some(c) = upd.content {
                            m.content = c;
                        }
                        if upd.edited_timestamp.is_some() {
                            m.edited_timestamp = upd.edited_timestamp;
                        }
                    }
                }
                NetEvent::Gateway(GatewayEvent::MessageDelete(del)) => {
                    if let Some(list) = self.messages.get_mut(&del.channel_id) {
                        list.retain(|m| m.id != del.id);
                    }
                }
                NetEvent::Gateway(GatewayEvent::Error(e)) => self.last_error = Some(e),
                NetEvent::Messages { channel_id, messages } => {
                    self.messages.insert(channel_id, messages);
                }
                NetEvent::Image { url, bytes } => {
                    if let Ok(img) = image::load_from_memory(&bytes) {
                        let rgba = img.to_rgba8();
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                        let tex = ctx.load_texture(url.clone(), color, egui::TextureOptions::LINEAR);
                        self.textures.insert(url, tex);
                    }
                }
                NetEvent::Error(e) => self.last_error = Some(e),
            }
        }
    }

    fn add_dm(&mut self, ch: Channel) {
        if !self.dms.iter().any(|d| d.id == ch.id) {
            self.dms.push(ch);
        }
    }

    /// Asks the network layer for any avatars and server icons we haven't loaded yet.
    fn request_images(&mut self) {
        let mut urls: Vec<String> = Vec::new();
        for g in &self.guilds {
            urls.extend(g.icon_url());
        }
        for dm in &self.dms {
            urls.extend(dm.recipients.iter().map(|u| u.avatar_url()));
        }
        if let Some(me) = &self.me {
            urls.push(me.avatar_url());
        }
        if let Some(list) = self.selected_channel.as_ref().and_then(|c| self.messages.get(c)) {
            urls.extend(list.iter().map(|m| m.avatar_url()));
        }
        for url in urls {
            if !self.textures.contains_key(&url) && self.requested.insert(url.clone()) {
                let _ = self.cmd_tx.send(NetCommand::FetchImage(url));
            }
        }
    }

    /// Title for the header and composer: "# channel" for servers, the other person for DMs.
    fn channel_title(&self, channel_id: &str) -> String {
        if let Some(dm) = self.dms.iter().find(|d| d.id == channel_id) {
            return dm.dm_title();
        }
        self.guilds
            .iter()
            .flat_map(|g| g.channels.iter())
            .find(|c| c.id == channel_id)
            .map(|c| format!("# {}", c.name.clone().unwrap_or_default()))
            .unwrap_or_default()
    }

    // ---------- Screens ----------

    fn login_ui(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(Frame::none().fill(CHAT_BG))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.2);
                    ui.heading("Rustcord");

                    if self.use_relay {
                        ui.label(RichText::new("Sign in to your Rustcord account.").color(MUTED));
                        ui.add_space(8.0);

                        ui.add(
                            egui::TextEdit::singleline(&mut self.relay_url)
                                .hint_text("Server address")
                                .desired_width(360.0),
                        );
                        ui.add_space(4.0);
                        let user = ui.add(
                            egui::TextEdit::singleline(&mut self.relay_user)
                                .hint_text("Username")
                                .desired_width(360.0),
                        );
                        ui.add_space(4.0);
                        let pass = ui.add(
                            egui::TextEdit::singleline(&mut self.relay_pass)
                                .password(true)
                                .hint_text("Password")
                                .desired_width(360.0),
                        );
                        ui.add_space(4.0);
                        ui.checkbox(&mut self.stay_signed_in, "Stay signed in on this Mac");
                        ui.add_space(6.0);

                        let enter = (user.lost_focus() || pass.lost_focus())
                            && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let busy = self.relay_busy;
                        let mut log_in = false;
                        let mut create = false;
                        ui.vertical_centered(|ui| {
                            ui.horizontal(|ui| {
                                log_in = ui.add_enabled(!busy, egui::Button::new("Log in")).clicked();
                                create = ui
                                    .add_enabled(!busy, egui::Button::new("Create account"))
                                    .clicked();
                            });
                        });

                        if (log_in || create || enter) && !busy {
                            let url = self.relay_url.trim().to_string();
                            let username = self.relay_user.trim().to_string();
                            let password = self.relay_pass.clone();
                            if url.is_empty() {
                                self.last_error = Some("Enter the server address".into());
                            } else if username.is_empty() || password.is_empty() {
                                self.last_error = Some("Enter your username and password".into());
                            } else {
                                // Enter means log in; the button decides for creating accounts.
                                let register = create;
                                self.relay_busy = true;
                                self.last_error = None;
                                self.status = if register {
                                    "Creating your account…".into()
                                } else {
                                    "Logging in…".into()
                                };
                                self.spawn_relay(move || relay::login(url, username, password, register));
                            }
                        }

                        ui.add_space(8.0);
                        if ui.small_button("Use a Discord bot token instead").clicked() {
                            self.use_relay = false;
                            self.last_error = None;
                        }
                    } else {
                        ui.label(RichText::new("Paste a Discord bot token to connect.").color(MUTED));
                        ui.add_space(8.0);

                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.token_input)
                                .password(true)
                                .hint_text("Bot token")
                                .desired_width(360.0),
                        );
                        ui.add_space(4.0);
                        ui.checkbox(&mut self.remember, "Remember token in the macOS Keychain");
                        ui.add_space(4.0);
                        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let clicked = ui
                            .add_enabled(!self.logged_in, egui::Button::new("Connect"))
                            .clicked();

                        if (clicked || enter) && !self.logged_in && !self.token_input.trim().is_empty() {
                            let token = self.token_input.trim().to_string();
                            self.last_error = None;
                            if self.remember {
                                match save_token(&token) {
                                    Ok(()) => self.has_saved_token = true,
                                    Err(e) => self.last_error = Some(e),
                                }
                            }
                            let _ = self.cmd_tx.send(NetCommand::Connect(token));
                            self.logged_in = true;
                            self.status = "Connecting…".into();
                        }

                        ui.add_space(8.0);
                        if ui.small_button("Use a Rustcord account instead").clicked() {
                            self.use_relay = true;
                            self.last_error = None;
                        }
                    }

                    ui.add_space(8.0);
                    if self.relay_busy {
                        ui.label(RichText::new(self.status.as_str()).color(MUTED));
                    } else if self.use_relay {
                        ui.label(RichText::new(self.status.as_str()).small().color(MUTED));
                    } else {
                        ui.label(RichText::new(self.status.as_str()).color(MUTED));
                    }
                    if let Some(e) = &self.last_error {
                        ui.colored_label(Color32::LIGHT_RED, e.as_str());
                    }
                });
            });
    }

    fn main_ui(&mut self, ctx: &egui::Context) {
        // Actions gathered during layout, applied afterwards to avoid borrow conflicts.
        let mut new_view: Option<View> = None;
        let mut new_channel: Option<String> = None;
        let mut to_send: Option<(String, String)> = None;
        let mut edit_start: Option<Editing> = None;
        let mut edit_save: Option<(String, String)> = None; // (message id, new text)
        let mut edit_cancel = false;
        let mut delete_req: Option<String> = None;
        let mut add_server_click = false;
        let mut profile_click = false;
        let mut sign_out_click = false;
        let mut join_req: Option<String> = None;
        let mut profile_req: Option<(String, String, String)> = None;
        let relay_mode = self.relay_session.is_some();
        let mut voice_click: Option<String> = None;
        let mut mute_click = false;
        let mut deafen_click = false;
        let mut leave_click = false;
        let voice_channel = self.voice_channel.clone();

        // Bottom status bar
        egui::TopBottomPanel::bottom("status")
            .frame(Frame::none().fill(SERVER_BG).inner_margin(Margin::symmetric(10.0, 4.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.status.as_str()).small().color(MUTED));
                    if let Some(e) = &self.last_error {
                        ui.colored_label(Color32::LIGHT_RED, e.as_str());
                    }
                });
            });

        // Message composer
        egui::TopBottomPanel::bottom("composer")
            .frame(Frame::none().fill(CHAT_BG).inner_margin(Margin::symmetric(16.0, 12.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let enabled = self.selected_channel.is_some();
                    let hint = self
                        .selected_channel
                        .as_deref()
                        .map(|c| format!("Message {}", self.channel_title(c)))
                        .unwrap_or_else(|| "Select a conversation".into());
                    let width = (ui.available_width() - 80.0).max(100.0);
                    let resp = ui.add_enabled(
                        enabled,
                        egui::TextEdit::singleline(&mut self.composer)
                            .hint_text(hint)
                            .margin(Margin::symmetric(10.0, 8.0))
                            .desired_width(width),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let clicked = ui.add_enabled(enabled, egui::Button::new("Send")).clicked();

                    if (enter || clicked) && enabled {
                        let content = self.composer.trim().to_string();
                        if let (false, Some(ch)) = (content.is_empty(), self.selected_channel.clone()) {
                            to_send = Some((ch, content));
                            self.composer.clear();
                            resp.request_focus();
                        }
                    }
                });
            });

        // Server rail (far left): Home (DMs) first, then server icons
        egui::SidePanel::left("servers")
            .exact_width(72.0)
            .resizable(false)
            .frame(Frame::none().fill(SERVER_BG).inner_margin(Margin::symmetric(0.0, 10.0)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        let home_selected = self.view == View::Home;
                        let resp = server_button(ui, None, "DM", ACCENT, home_selected);
                        if resp.on_hover_text("Direct messages").clicked() {
                            new_view = Some(View::Home);
                        }
                        ui.add_space(8.0);

                        if relay_mode {
                            let add = ui
                                .add_sized([48.0, 48.0], egui::Button::new(RichText::new("+").size(22.0)))
                                .on_hover_text("Add a server");
                            if add.clicked() {
                                add_server_click = true;
                            }
                            ui.add_space(8.0);
                        }
                        ui.separator();
                        ui.add_space(8.0);

                        for g in &self.guilds {
                            let selected = self.view == View::Guild(g.id.clone());
                            let tex = g.icon_url().and_then(|u| self.textures.get(&u));
                            let resp = server_button(ui, tex, &g.name, color_for(&g.id), selected);
                            if resp.on_hover_text(g.name.as_str()).clicked() {
                                new_view = Some(View::Guild(g.id.clone()));
                            }
                            ui.add_space(8.0);
                        }
                    });
                });
            });

        // Sidebar: DM list or server channels, plus the user bar
        egui::SidePanel::left("channels")
            .default_width(240.0)
            .resizable(true)
            .frame(Frame::none().fill(CHANNEL_BG).inner_margin(Margin::symmetric(8.0, 8.0)))
            .show(ctx, |ui| {
                let selected = self.selected_channel.as_deref();
                match &self.view {
                    View::Home => {
                        ui.label(RichText::new("DIRECT MESSAGES").small().strong().color(MUTED));
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            if relay_mode {
                                ui.label(
                                    RichText::new("Direct messages are not available with a Rustcord account yet.")
                                        .color(MUTED),
                                );
                            } else if self.dms.is_empty() {
                                ui.label(RichText::new("No direct messages yet").color(MUTED));
                            }
                            for dm in &self.dms {
                                let sel = selected == Some(dm.id.as_str());
                                if channel_row(ui, &dm.dm_title(), sel).clicked() {
                                    new_channel = Some(dm.id.clone());
                                }
                            }
                        });
                    }
                    View::Guild(gid) => match self.guilds.iter().find(|g| &g.id == gid) {
                        None => {
                            ui.label("Loading server…");
                        }
                        Some(guild) => {
                            ui.label(RichText::new(guild.name.as_str()).strong().size(15.0));
                            ui.add_space(6.0);
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                if guild.channels.is_empty() {
                                    ui.label(RichText::new("Loading channels…").color(MUTED));
                                }
                                for row in layout_channels(&guild.channels) {
                                    match row {
                                        Row::Header(name) => {
                                            ui.add_space(10.0);
                                            ui.label(RichText::new(name.to_uppercase()).small().strong().color(MUTED));
                                        }
                                        Row::Chan(ch) if ch.is_text() => {
                                            let name = ch.name.clone().unwrap_or_default();
                                            let sel = selected == Some(ch.id.as_str());
                                            if channel_row(ui, &format!("# {name}"), sel).clicked() {
                                                new_channel = Some(ch.id.clone());
                                            }
                                        }
                                        Row::Chan(ch) if ch.is_voice() => {
                                            let name = ch.name.clone().unwrap_or_default();
                                            let in_call = voice_channel.as_deref() == Some(ch.id.as_str());
                                            if channel_row(ui, &format!("🔊 {name}"), in_call).clicked() {
                                                voice_click = Some(ch.id.clone());
                                            }
                                        }
                                        Row::Chan(_) => {}
                                    }
                                }
                            });
                        }
                    },
                }

                // User bar at the bottom. Voice controls are placeholders until voice is built.
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.add_enabled(false, egui::Button::new("📷"))
                            .on_disabled_hover_text("Camera: not implemented yet");
                        ui.add_enabled(false, egui::Button::new("🎧"))
                            .on_disabled_hover_text("Deafen: not implemented yet");
                        ui.add_enabled(false, egui::Button::new("🎙"))
                            .on_disabled_hover_text("Mute: not implemented yet");
                    });
                    let (name, avatar_url, id) = match &self.me {
                        Some(u) => (u.display_name().to_string(), u.avatar_url(), u.id.clone()),
                        None => ("Not signed in".into(), String::new(), String::new()),
                    };
                    let about = self.profile.as_ref().map(|p| p.description.clone()).unwrap_or_default();
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
                        draw_avatar(ui, rect, self.textures.get(&avatar_url), &name, color_for(&id), 16.0);
                        let label = ui.label(RichText::new(name).strong());
                        if !about.is_empty() {
                            label.on_hover_text(about);
                        }
                    });
                    if let Some(v) = &self.voice {
                        let muted = v.muted.load(std::sync::atomic::Ordering::Relaxed);
                        let deaf = v.deafened.load(std::sync::atomic::Ordering::Relaxed);
                        let status_text = v.status.lock().unwrap().clone();
                        ui.label(RichText::new(status_text).small().color(MUTED));
                        ui.horizontal(|ui| {
                            if ui.small_button(if muted { "Unmute" } else { "Mute" }).clicked() {
                                mute_click = true;
                            }
                            if ui.small_button(if deaf { "Undeafen" } else { "Deafen" }).clicked() {
                                deafen_click = true;
                            }
                            if ui.small_button("Leave").clicked() {
                                leave_click = true;
                            }
                        });
                    }
                    if relay_mode {
                        ui.horizontal(|ui| {
                            if ui.small_button("Edit profile").clicked() {
                                profile_click = true;
                            }
                            if ui.small_button("Sign out").clicked() {
                                sign_out_click = true;
                            }
                        });
                    }
                    if !relay_mode
                        && self.has_saved_token
                        && ui
                            .small_button("Forget saved token")
                            .on_hover_text("Removes the token from the Keychain")
                            .clicked()
                    {
                        forget_token();
                        self.has_saved_token = false;
                    }
                    ui.separator();
                });
            });

        // Chat area
        egui::CentralPanel::default()
            .frame(Frame::none().fill(CHAT_BG).inner_margin(Margin::symmetric(16.0, 10.0)))
            .show(ctx, |ui| {
                let Some(cid) = self.selected_channel.clone() else {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("Select a conversation to start chatting").color(MUTED));
                    });
                    return;
                };
                ui.label(RichText::new(self.channel_title(&cid)).strong().size(16.0));
                ui.separator();

                let my_id = self.me.as_ref().map(|u| u.id.clone());
                match self.messages.get(&cid) {
                    None => {
                        ui.label(RichText::new("Loading messages…").color(MUTED));
                    }
                    Some(list) => {
                        egui::ScrollArea::vertical()
                            .stick_to_bottom(true)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for m in list {
                                    ui.horizontal_top(|ui| {
                                        let (rect, _) =
                                            ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                                        let url = m.avatar_url();
                                        draw_avatar(
                                            ui,
                                            rect,
                                            self.textures.get(&url),
                                            m.author.display_name(),
                                            color_for(&m.author.id),
                                            20.0,
                                        );
                                        ui.vertical(|ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.label(RichText::new(m.author.display_name()).strong());
                                                ui.label(RichText::new(short_time(&m.timestamp)).small().color(MUTED));
                                                if m.edited_timestamp.is_some() {
                                                    ui.label(RichText::new("(edited)").small().color(MUTED));
                                                }
                                            });

                                            // Editing this message: inline text box.
                                            if let Some(ed) =
                                                self.editing.as_mut().filter(|e| e.message_id == m.id)
                                            {
                                                let resp = ui.add(
                                                    egui::TextEdit::singleline(&mut ed.text)
                                                        .desired_width(f32::INFINITY),
                                                );
                                                let enter = resp.lost_focus()
                                                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                                ui.horizontal(|ui| {
                                                    if ui.button("Save").clicked() || enter {
                                                        edit_save =
                                                            Some((m.id.clone(), ed.text.trim().to_string()));
                                                    }
                                                    if ui.button("Cancel").clicked() || esc {
                                                        edit_cancel = true;
                                                    }
                                                });
                                            } else {
                                                let body = if m.content.is_empty() {
                                                    "[attachment or embed]"
                                                } else {
                                                    m.content.as_str()
                                                };
                                                let r = ui.add(
                                                    egui::Label::new(RichText::new(body).color(TEXT))
                                                        .wrap()
                                                        .sense(Sense::click()),
                                                );
                                                r.context_menu(|ui| {
                                                    if my_id.as_deref() == Some(m.author.id.as_str()) {
                                                        if ui.button("Edit").clicked() {
                                                            edit_start = Some(Editing {
                                                                channel_id: cid.clone(),
                                                                message_id: m.id.clone(),
                                                                text: m.content.clone(),
                                                            });
                                                            ui.close_menu();
                                                        }
                                                        if ui.button("Delete").clicked() {
                                                            delete_req = Some(m.id.clone());
                                                            ui.close_menu();
                                                        }
                                                    }
                                                    if ui.button("Copy text").clicked() {
                                                        ui.ctx().copy_text(m.content.clone());
                                                        ui.close_menu();
                                                    }
                                                });
                                            }

                                            for a in &m.attachments {
                                                ui.hyperlink_to(format!("📎 {}", a.filename), a.url.as_str());
                                            }
                                        });
                                    });
                                    ui.add_space(10.0);
                                }
                            });
                    }
                }
            });

        // Add-a-server window (relay accounts)
        if self.add_server_open {
            let busy = self.relay_busy;
            let mut open = true;
            egui::Window::new("Add a server")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("The Rustcord bot must already be in the server. Ask the owner to add it first.");
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Then paste the server ID. In Discord: Settings → Advanced → turn on Developer Mode, \
                             right-click the server, and choose Copy Server ID.",
                        )
                        .small()
                        .color(MUTED),
                    );
                    ui.add_space(6.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.add_server_input)
                            .hint_text("Server ID")
                            .desired_width(300.0),
                    );
                    ui.add_space(6.0);
                    if ui.add_enabled(!busy, egui::Button::new("Add server")).clicked() {
                        let id: String = self.add_server_input.chars().filter(|c| c.is_ascii_digit()).collect();
                        if id.is_empty() {
                            self.last_error = Some("Paste the server ID (numbers only)".into());
                        } else {
                            join_req = Some(id);
                        }
                    }
                });
            self.add_server_open = open;
        }

        // Profile window (relay accounts)
        if self.profile_open {
            let mut open = true;
            let mut cancel = false;
            egui::Window::new("Your profile")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Display name");
                    ui.add(egui::TextEdit::singleline(&mut self.edit_name).desired_width(300.0));
                    ui.add_space(4.0);
                    ui.label("Avatar picture (an https:// image link, empty for none)");
                    ui.add(egui::TextEdit::singleline(&mut self.edit_avatar).desired_width(300.0));
                    ui.add_space(4.0);
                    ui.label("About me");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.edit_desc)
                            .desired_rows(3)
                            .desired_width(300.0),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            profile_req = Some((
                                self.edit_name.trim().to_string(),
                                self.edit_avatar.trim().to_string(),
                                self.edit_desc.trim().to_string(),
                            ));
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            self.profile_open = open && !cancel;
        }

        // Apply actions
        if let Some(id) = voice_click {
            if self.voice_channel.as_deref() == Some(id.as_str()) {
                self.voice = None;
                self.voice_channel = None;
            } else {
                match crate::voice::VoiceSession::start() {
                    Ok(session) => {
                        self.voice = Some(session);
                        self.voice_channel = Some(id);
                        self.last_error = None;
                    }
                    Err(e) => self.last_error = Some(format!("Voice: {e}")),
                }
            }
        }
        if let Some(v) = &self.voice {
            if mute_click {
                v.muted.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
            }
            if deafen_click {
                v.deafened.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if leave_click {
            self.voice = None;
            self.voice_channel = None;
        }
        if sign_out_click {
            self.sign_out();
            return;
        }
        if add_server_click {
            self.add_server_open = true;
            self.add_server_input.clear();
        }
        if profile_click {
            if let Some(p) = &self.profile {
                self.edit_name = p.display_name.clone();
                self.edit_avatar = p.avatar_url.clone().unwrap_or_default();
                self.edit_desc = p.description.clone();
            }
            self.profile_open = true;
        }
        if let Some(id) = join_req {
            if let Some((url, token)) = self.relay_session.clone() {
                self.relay_busy = true;
                self.last_error = None;
                self.status = "Adding server…".into();
                self.spawn_relay(move || relay::join(url, token, id));
            }
        }
        if let Some((name, avatar, desc)) = profile_req {
            if let Some((url, token)) = self.relay_session.clone() {
                self.relay_busy = true;
                self.spawn_relay(move || relay::update_profile(url, token, name, avatar, desc));
            }
        }
        if let Some(view) = new_view {
            if self.view != view {
                self.view = view.clone();
                if let View::Guild(gid) = &view {
                    if relay_mode {
                        self.selected_channel = None;
                        self.request_channels(gid.clone());
                    } else {
                        let first = self.guilds.iter().find(|g| &g.id == gid).and_then(first_text_channel);
                        if let Some(cid) = first {
                            self.open_channel(cid);
                        }
                    }
                }
            }
        }
        if let Some(cid) = new_channel {
            self.open_channel(cid);
        }
        if let Some((channel_id, content)) = to_send {
            if relay_mode {
                if let (Some((url, token)), View::Guild(gid)) = (self.relay_session.clone(), self.view.clone()) {
                    self.spawn_relay(move || relay::send(url, token, gid, channel_id, content));
                } else {
                    self.last_error = Some("Open a server to send messages".into());
                }
            } else {
                let _ = self.cmd_tx.send(NetCommand::Send { channel_id, content });
            }
        }
        if let Some(e) = edit_start {
            self.editing = Some(e);
        }
        if edit_cancel {
            self.editing = None;
        }
        if let Some((message_id, text)) = edit_save {
            if let (Some(ed), false) = (self.editing.take(), text.is_empty()) {
                let _ = self.cmd_tx.send(NetCommand::Edit {
                    channel_id: ed.channel_id,
                    message_id,
                    content: text,
                });
            }
        }
        if let Some(message_id) = delete_req {
            if let Some(channel_id) = self.selected_channel.clone() {
                let _ = self.cmd_tx.send(NetCommand::Delete { channel_id, message_id });
            }
        }
    }

    /// Banner shown across the top when a newer release exists.
    fn update_banner(&mut self, ctx: &egui::Context) {
        let Some(info) = self.update_available.clone() else {
            return;
        };
        egui::TopBottomPanel::top("update_banner")
            .frame(Frame::none().fill(ACCENT).inner_margin(Margin::symmetric(12.0, 6.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let text = match &self.update_msg {
                        Some(m) => m.clone(),
                        None => format!("Rustcord {} is available.", info.version),
                    };
                    ui.label(RichText::new(text).color(Color32::WHITE));
                    if self.update_busy {
                        ui.add(egui::Spinner::new());
                    } else if ui.button("Update").clicked() {
                        self.update_busy = true;
                        self.update_msg = Some("Downloading the update…".into());
                        let tx = self.update_msg_tx.clone();
                        let info = info.clone();
                        std::thread::spawn(move || {
                            let msg = match update::download_and_open(&info) {
                                Ok(path) => format!("Installer saved to {}. Finish the install, then reopen Rustcord.", path.display()),
                                Err(e) => e,
                            };
                            let _ = tx.send(msg);
                        });
                    }
                    if ui.small_button("Later").clicked() {
                        self.update_available = None;
                    }
                });
            });
    }

    fn open_channel(&mut self, channel_id: String) {
        self.editing = None;
        self.selected_channel = Some(channel_id.clone());
        if self.relay_session.is_some() {
            if let View::Guild(gid) = self.view.clone() {
                let gid = gid.clone();
                self.request_messages(&gid, &channel_id);
            }
        } else if !self.messages.contains_key(&channel_id) {
            let _ = self.cmd_tx.send(NetCommand::FetchMessages(channel_id));
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.styled {
            apply_theme(ctx);
            self.styled = true;
        }
        self.ctx = Some(ctx.clone());

        // Tray: show the window or quit when asked.
        if let Some(tray) = &self.tray {
            for action in tray.poll() {
                match action {
                    crate::tray::TrayAction::Show => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    crate::tray::TrayAction::Quit => {
                        self.quitting = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
        }
        // Closing the window hides it instead, so Rustcord keeps running in the background.
        if self.tray.is_some() && !self.quitting && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        // Keep checking the tray while hidden.
        ctx.request_repaint_after(Duration::from_millis(500));

        // Once at startup: look for a newer release.
        if !self.update_checked {
            self.update_checked = true;
            let (tx, rx) = channel();
            self.update_rx = rx;
            update::check_in_background(tx, ctx.clone());
        }
        while let Ok(info) = self.update_rx.try_recv() {
            self.update_available = Some(info);
        }
        while let Ok(msg) = self.update_msg_rx.try_recv() {
            self.update_msg = Some(msg);
            self.update_busy = false;
        }
        self.update_banner(ctx);

        // Re-sign-in with a saved session, once the UI is ready to receive the result.
        if let Some((url, token)) = self.pending_restore.take() {
            self.spawn_relay(move || relay::restore(url, token));
        }

        self.drain_events(ctx);
        self.request_images();

        // Relay mode has no live connection yet, so refresh the open channel regularly.
        if self.connected && self.relay_session.is_some() {
            if let (Some(cid), View::Guild(gid)) = (self.selected_channel.clone(), self.view.clone()) {
                let due = self.last_poll.map_or(true, |t| t.elapsed() >= POLL_EVERY);
                if due {
                    self.last_poll = Some(Instant::now());
                    self.request_messages(&gid, &cid);
                }
                ctx.request_repaint_after(POLL_EVERY);
            }
        }

        if self.connected {
            self.main_ui(ctx);
        } else {
            self.login_ui(ctx);
        }
    }
}

fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = CHAT_BG;
    v.window_fill = CHANNEL_BG;
    v.extreme_bg_color = INPUT_BG;
    v.faint_bg_color = CHANNEL_BG;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT;
    v.hyperlink_color = Color32::from_rgb(0, 168, 252);
    v.widgets.noninteractive.bg_fill = CHAT_BG;
    v.widgets.inactive.bg_fill = INPUT_BG;
    v.widgets.inactive.weak_bg_fill = INPUT_BG;
    v.widgets.hovered.bg_fill = HOVER;
    v.widgets.hovered.weak_bg_fill = HOVER;
    v.widgets.active.bg_fill = SELECTED;
    v.widgets.active.weak_bg_fill = SELECTED;
    v.widgets.inactive.rounding = 4.0.into();
    v.widgets.hovered.rounding = 4.0.into();
    v.widgets.active.rounding = 4.0.into();
    ctx.set_visuals(v);
}

/// First text channel of a server in sidebar order, if any.
fn first_text_channel(guild: &Guild) -> Option<String> {
    layout_channels(&guild.channels).into_iter().find_map(|r| match r {
        Row::Chan(c) if c.is_text() => Some(c.id.clone()),
        _ => None,
    })
}

/// A channel row in the sidebar, highlighted when selected.
fn channel_row(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
    let fill = if selected { SELECTED } else { Color32::TRANSPARENT };
    let color = if selected { Color32::WHITE } else { MUTED };
    let btn = egui::Button::new(RichText::new(text).color(color))
        .fill(fill)
        .min_size(Vec2::new(ui.available_width(), 28.0));
    ui.add(btn)
}

/// Round server button for the left rail. Shows the server icon when loaded, otherwise initials.
fn server_button(
    ui: &mut egui::Ui,
    tex: Option<&TextureHandle>,
    name: &str,
    color: Color32,
    selected: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::click());
    if selected || resp.hovered() {
        let h = if selected { 40.0 } else { 20.0 };
        let bar = Rect::from_center_size(
            egui::pos2(rect.left() - 10.0, rect.center().y),
            Vec2::new(4.0, h),
        );
        ui.painter().rect_filled(bar, 2.0, Color32::WHITE);
    }
    let rounding = if selected { 14.0 } else { 24.0 };
    let label = if name.len() <= 3 {
        name.to_string()
    } else {
        name.split_whitespace()
            .filter_map(|w| w.chars().next())
            .take(3)
            .collect()
    };
    draw_avatar(ui, rect, tex, &label, color, rounding);
    resp
}

/// Draws an image (rounded) or, if none, a coloured tile with initials.
fn draw_avatar(
    ui: &egui::Ui,
    rect: Rect,
    tex: Option<&TextureHandle>,
    label: &str,
    color: Color32,
    rounding: f32,
) {
    match tex {
        Some(t) => {
            egui::Image::new(egui::load::SizedTexture::new(t.id(), rect.size()))
                .rounding(rounding)
                .paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, rounding, color);
            let letter: String = label
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".into());
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                letter,
                FontId::proportional(rect.height() * 0.42),
                Color32::WHITE,
            );
        }
    }
}

/// Stable colour per ID so each server/user gets the same fallback tile every time.
fn color_for(id: &str) -> Color32 {
    const PALETTE: [Color32; 6] = [
        Color32::from_rgb(88, 101, 242),
        Color32::from_rgb(87, 242, 135),
        Color32::from_rgb(254, 231, 92),
        Color32::from_rgb(235, 69, 158),
        Color32::from_rgb(237, 66, 69),
        Color32::from_rgb(0, 168, 252),
    ];
    let h = id.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
    PALETTE[h % PALETTE.len()]
}

/// Orders channels: uncategorised first, then each category with its children.
fn layout_channels(channels: &[Channel]) -> Vec<Row<'_>> {
    let mut rows = Vec::new();

    let mut loose: Vec<&Channel> = channels
        .iter()
        .filter(|c| c.kind != 4 && c.parent_id.is_none())
        .collect();
    loose.sort_by_key(|c| c.position);
    rows.extend(loose.into_iter().map(Row::Chan));

    let mut categories: Vec<&Channel> = channels.iter().filter(|c| c.kind == 4).collect();
    categories.sort_by_key(|c| c.position);
    for cat in categories {
        rows.push(Row::Header(cat.name.as_deref().unwrap_or("category")));
        let mut kids: Vec<&Channel> = channels
            .iter()
            .filter(|c| c.parent_id.as_deref() == Some(cat.id.as_str()))
            .collect();
        kids.sort_by_key(|c| c.position);
        rows.extend(kids.into_iter().map(Row::Chan));
    }
    rows
}

/// "2026-11-14T10:32:05.123000+00:00" -> "10:32" (UTC)
fn short_time(ts: &str) -> &str {
    ts.get(11..16).unwrap_or("")
}
