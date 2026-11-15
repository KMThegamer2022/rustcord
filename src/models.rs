//! Discord API data types (the subset Rustcord uses).

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    /// Full avatar link set by the relay (not from Discord).
    #[serde(skip)]
    pub avatar_override: Option<String>,
}

impl User {
    pub fn display_name(&self) -> &str {
        self.global_name.as_deref().unwrap_or(&self.username)
    }

    /// Custom avatar if set, otherwise Discord's default avatar for this user.
    pub fn avatar_url(&self) -> String {
        if let Some(url) = &self.avatar_override {
            return url.clone();
        }
        match &self.avatar {
            Some(hash) => format!("https://cdn.discordapp.com/avatars/{}/{hash}.png?size=64", self.id),
            None => {
                let idx = self.id.parse::<u64>().map(|id| (id >> 22) % 6).unwrap_or(0);
                format!("https://cdn.discordapp.com/embed/avatars/{idx}.png")
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Channel {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(default)]
    pub position: i64,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub recipients: Vec<User>,
}

impl Channel {
    /// 0 = GUILD_TEXT, 5 = GUILD_ANNOUNCEMENT
    pub fn is_text(&self) -> bool {
        matches!(self.kind, 0 | 5)
    }

    /// 2 = GUILD_VOICE, 13 = GUILD_STAGE_VOICE
    pub fn is_voice(&self) -> bool {
        matches!(self.kind, 2 | 13)
    }

    /// 1 = DM, 3 = group DM
    pub fn is_dm(&self) -> bool {
        matches!(self.kind, 1 | 3)
    }

    /// Name shown in the DM list.
    pub fn dm_title(&self) -> String {
        if let Some(name) = self.name.as_deref().filter(|n| !n.is_empty()) {
            return name.to_string();
        }
        let names: Vec<&str> = self.recipients.iter().map(|u| u.display_name()).collect();
        if names.is_empty() {
            "Direct message".into()
        } else {
            names.join(", ")
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Guild {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub channels: Vec<Channel>,
}

impl Guild {
    pub fn icon_url(&self) -> Option<String> {
        self.icon
            .as_ref()
            .map(|hash| format!("https://cdn.discordapp.com/icons/{}/{hash}.png?size=96", self.id))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Attachment {
    pub filename: String,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub author: User,
    #[serde(default)]
    pub content: String,
    pub timestamp: String,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Avatar of the real sender when the message came through the relay.
    #[serde(default)]
    pub relay_avatar: Option<String>,
}

impl Message {
    /// Avatar to show: the relay's copy when present, otherwise Discord's.
    pub fn avatar_url(&self) -> String {
        self.relay_avatar.clone().unwrap_or_else(|| self.author.avatar_url())
    }
}

/// MESSAGE_UPDATE: only the changed fields are guaranteed to be present.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageUpdate {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
}

/// MESSAGE_DELETE
#[derive(Debug, Clone, Deserialize)]
pub struct MessageDelete {
    pub id: String,
    pub channel_id: String,
}
