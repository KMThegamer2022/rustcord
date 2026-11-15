//! Client for the Rustcord relay server (accounts, profiles, servers, messages).
//! Each call runs on its own thread and reports back as a `RelayEvent`, which
//! the UI picks up on its next frame.

use crate::models::{Channel, Guild, Message, User};
use serde_json::{json, Value};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Profile {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub description: String,
}

impl Profile {
    fn from_json(v: &Value) -> Option<Profile> {
        Some(Profile {
            id: v["id"].as_i64()?,
            username: v["username"].as_str()?.to_string(),
            display_name: v["display_name"].as_str()?.to_string(),
            avatar_url: v["avatar_url"].as_str().map(String::from),
            description: v["description"].as_str().unwrap_or("").to_string(),
        })
    }

    /// The user shown in the sidebar and used for "me" checks.
    pub fn to_user(&self) -> User {
        User {
            id: format!("relay-{}", self.id),
            username: self.username.clone(),
            global_name: Some(self.display_name.clone()),
            avatar: None,
            avatar_override: self.avatar_url.clone(),
        }
    }
}

pub enum RelayEvent {
    /// Signed in (or restored a saved session).
    LoggedIn { url: String, token: String, profile: Profile },
    Profile(Profile),
    Guilds(Vec<Guild>),
    Joined(Guild),
    Channels { guild_id: String, channels: Vec<Channel> },
    Messages { channel_id: String, messages: Vec<Message> },
    Sent { guild_id: String, channel_id: String },
    /// The saved session is no longer valid; the user must sign in again.
    SessionExpired,
    Error(String),
}

pub struct CallError {
    pub status: u16,
    pub message: String,
}

fn api(base: &str, path: &str) -> String {
    format!("{}/api{}", base.trim_end_matches('/'), path)
}

fn call(method: reqwest::Method, url: String, token: Option<&str>, body: Option<Value>) -> Result<Value, CallError> {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .user_agent("Rustcord/0.1 (client)")
            .timeout(Duration::from_secs(15))
            .build()
            .expect("failed to build HTTP client")
    });
    let mut req = client.request(method, &url);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().map_err(|_| CallError {
        status: 0,
        message: format!("Could not reach the relay at {url}. Is it running?"),
    })?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    let json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status.is_success() {
        Ok(json)
    } else {
        let message = json["error"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| format!("The relay returned {status}"));
        Err(CallError { status: status.as_u16(), message })
    }
}

/// A failed call that needs a fresh sign-in turns into SessionExpired.
fn session_failure(e: CallError) -> RelayEvent {
    if e.status == 401 {
        RelayEvent::SessionExpired
    } else {
        RelayEvent::Error(e.message)
    }
}

fn fetch_profile(base: &str, token: &str) -> Result<Profile, CallError> {
    let v = call(reqwest::Method::GET, api(base, "/me"), Some(token), None)?;
    Profile::from_json(&v).ok_or(CallError { status: 0, message: "Unexpected profile from the relay".into() })
}

/// Creates an account or signs in, then loads the profile.
pub fn login(base: String, username: String, password: String, register: bool) -> RelayEvent {
    let path = if register { "/register" } else { "/login" };
    let body = json!({ "username": username, "password": password });
    let auth = match call(reqwest::Method::POST, api(&base, path), None, Some(body)) {
        Ok(v) => v,
        Err(e) => return RelayEvent::Error(e.message),
    };
    let Some(token) = auth["token"].as_str().map(String::from) else {
        return RelayEvent::Error("The relay did not return a session".into());
    };
    match fetch_profile(&base, &token) {
        Ok(profile) => RelayEvent::LoggedIn { url: base, token, profile },
        Err(e) => RelayEvent::Error(e.message),
    }
}

/// Re-checks a saved session at startup.
pub fn restore(base: String, token: String) -> RelayEvent {
    match fetch_profile(&base, &token) {
        Ok(profile) => RelayEvent::LoggedIn { url: base, token, profile },
        Err(e) => session_failure(e),
    }
}

pub fn update_profile(
    base: String,
    token: String,
    display_name: String,
    avatar_url: String,
    description: String,
) -> RelayEvent {
    let body = json!({
        "display_name": display_name,
        "avatar_url": avatar_url,
        "description": description,
    });
    match call(reqwest::Method::POST, api(&base, "/profile"), Some(&token), Some(body)) {
        Ok(v) => match Profile::from_json(&v) {
            Some(p) => RelayEvent::Profile(p),
            None => RelayEvent::Error("Unexpected profile from the relay".into()),
        },
        Err(e) => session_failure(e),
    }
}

fn guild_from_json(v: &Value) -> Option<Guild> {
    Some(Guild {
        id: v["id"].as_str()?.to_string(),
        name: v["name"].as_str().unwrap_or("Server").to_string(),
        icon: v["icon"].as_str().map(String::from),
        channels: Vec::new(),
    })
}

pub fn guilds(base: String, token: String) -> RelayEvent {
    match call(reqwest::Method::GET, api(&base, "/guilds"), Some(&token), None) {
        Ok(v) => RelayEvent::Guilds(
            v["guilds"].as_array().map(|a| a.iter().filter_map(guild_from_json).collect()).unwrap_or_default(),
        ),
        Err(e) => session_failure(e),
    }
}

pub fn join(base: String, token: String, guild_id: String) -> RelayEvent {
    match call(
        reqwest::Method::POST,
        api(&base, &format!("/guilds/{guild_id}/join")),
        Some(&token),
        None,
    ) {
        Ok(v) => match guild_from_json(&v) {
            Some(g) => RelayEvent::Joined(g),
            None => RelayEvent::Error("Unexpected server from the relay".into()),
        },
        Err(e) => session_failure(e),
    }
}

pub fn channels(base: String, token: String, guild_id: String) -> RelayEvent {
    match call(
        reqwest::Method::GET,
        api(&base, &format!("/guilds/{guild_id}/channels")),
        Some(&token),
        None,
    ) {
        Ok(v) => {
            let channels = v["channels"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|c| {
                            Some(Channel {
                                id: c["id"].as_str()?.to_string(),
                                name: c["name"].as_str().map(String::from),
                                kind: c["type"].as_u64().unwrap_or(0) as u8,
                                position: c["position"].as_i64().unwrap_or(0),
                                parent_id: None,
                                recipients: Vec::new(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            RelayEvent::Channels { guild_id, channels }
        }
        Err(e) => session_failure(e),
    }
}

pub fn messages(base: String, token: String, guild_id: String, channel_id: String) -> RelayEvent {
    let path = format!("/guilds/{guild_id}/channels/{channel_id}/messages");
    match call(reqwest::Method::GET, api(&base, &path), Some(&token), None) {
        Ok(v) => match serde_json::from_value::<Vec<Message>>(v["messages"].clone()) {
            // Discord sends newest first; the UI shows oldest first.
            Ok(mut list) => {
                list.reverse();
                RelayEvent::Messages { channel_id, messages: list }
            }
            Err(e) => RelayEvent::Error(format!("Could not read messages: {e}")),
        },
        Err(e) => session_failure(e),
    }
}

pub fn send(base: String, token: String, guild_id: String, channel_id: String, content: String) -> RelayEvent {
    let path = format!("/guilds/{guild_id}/channels/{channel_id}/messages");
    match call(
        reqwest::Method::POST,
        api(&base, &path),
        Some(&token),
        Some(json!({ "content": content })),
    ) {
        Ok(_) => RelayEvent::Sent { guild_id, channel_id },
        Err(e) => session_failure(e),
    }
}

/// Runs `f` on a background thread and sends its result to the UI.
pub fn spawn<F>(tx: Sender<RelayEvent>, ctx: eframe::egui::Context, f: F)
where
    F: FnOnce() -> RelayEvent + Send + 'static,
{
    std::thread::spawn(move || {
        let event = f();
        let _ = tx.send(event);
        ctx.request_repaint();
    });
}

#[cfg(test)]
mod tests {
    //! Runs the client's real relay code against a relay started on RELAY_TEST_URL.
    //! Skipped (passes) when that variable is not set.
    use super::*;

    fn base() -> Option<String> {
        std::env::var("RELAY_TEST_URL").ok()
    }

    #[test]
    fn account_flow() {
        let Some(url) = base() else { return };
        let user = format!("tester{}", std::process::id());

        // Wrong password and missing fields show the relay's message.
        match login(url.clone(), user.clone(), "password123".into(), true) {
            RelayEvent::LoggedIn { .. } => {}
            RelayEvent::Error(e) => panic!("register failed: {e}"),
            _ => panic!("unexpected event"),
        }
        match login(url.clone(), user.clone(), "wrong-password".into(), false) {
            RelayEvent::Error(e) => assert_eq!(e, "Wrong username or password"),
            _ => panic!("expected wrong-password error"),
        }
        match login(url.clone(), user.clone(), "password123".into(), true) {
            RelayEvent::Error(e) => assert_eq!(e, "That username is taken"),
            _ => panic!("expected duplicate-name error"),
        }

        // Sign in, then use the session token.
        let (token, profile) = match login(url.clone(), user.clone(), "password123".into(), false) {
            RelayEvent::LoggedIn { token, profile, .. } => (token, profile),
            RelayEvent::Error(e) => panic!("login failed: {e}"),
            _ => panic!("unexpected event"),
        };
        assert_eq!(profile.username, user);

        match update_profile(url.clone(), token.clone(), "Test Name".into(), "https://example.com/a.png".into(), "Hello".into()) {
            RelayEvent::Profile(p) => {
                assert_eq!(p.display_name, "Test Name");
                assert_eq!(p.description, "Hello");
            }
            _ => panic!("profile update failed"),
        }
        match update_profile(url.clone(), token.clone(), "  ".into(), String::new(), String::new()) {
            RelayEvent::Error(e) => assert_eq!(e, "Display name must be 1-32 characters"),
            _ => panic!("expected name validation error"),
        }

        // Restoring a saved session works; a bad token asks for a fresh sign-in.
        assert!(matches!(restore(url.clone(), token.clone()), RelayEvent::LoggedIn { .. }));
        assert!(matches!(restore(url.clone(), "nope".into()), RelayEvent::SessionExpired));

        // A server the user hasn't joined is refused.
        match channels(url.clone(), token.clone(), "1111111111111111111".into()) {
            RelayEvent::Error(e) => assert_eq!(e, "You have not joined this server"),
            _ => panic!("expected membership error"),
        }
        match guilds(url.clone(), token) {
            RelayEvent::Guilds(list) => assert!(list.is_empty()),
            _ => panic!("guild list failed"),
        }
    }

    #[test]
    fn unreachable_relay_message() {
        let r = login("http://127.0.0.1:9".into(), "x".into(), "y".into(), false);
        match r {
            RelayEvent::Error(e) => assert!(e.contains("Could not reach the relay"), "{e}"),
            _ => panic!("expected connection error"),
        }
    }
}
