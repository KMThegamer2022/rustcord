//! Thin async client for the Discord REST API (v10).

use crate::models::{Message, User};
use anyhow::{bail, Result};
use reqwest::{Client, Response};
use serde::de::DeserializeOwned;
use serde_json::json;

const API: &str = "https://discord.com/api/v10";

#[derive(Clone)]
pub struct Rest {
    http: Client,
    auth: String,
}

impl Rest {
    pub fn new(token: &str) -> Self {
        let http = Client::builder()
            .user_agent("Rustcord/0.1 (native client)")
            .build()
            .expect("failed to build HTTP client");
        Self {
            http,
            auth: format!("Bot {token}"),
        }
    }

    #[allow(dead_code)]
    pub async fn me(&self) -> Result<User> {
        self.get("/users/@me").await
    }

    /// Returns messages oldest-first (the API returns newest-first).
    pub async fn messages(&self, channel_id: &str, limit: u8) -> Result<Vec<Message>> {
        let mut msgs: Vec<Message> = self
            .get(&format!("/channels/{channel_id}/messages?limit={limit}"))
            .await?;
        msgs.reverse();
        Ok(msgs)
    }

    pub async fn send(&self, channel_id: &str, content: &str) -> Result<Message> {
        let resp = self
            .http
            .post(format!("{API}/channels/{channel_id}/messages"))
            .header("Authorization", &self.auth)
            .json(&json!({ "content": content }))
            .send()
            .await?;
        parse(resp).await
    }

    /// Only works on messages sent by this bot.
    pub async fn edit(&self, channel_id: &str, message_id: &str, content: &str) -> Result<Message> {
        let resp = self
            .http
            .patch(format!("{API}/channels/{channel_id}/messages/{message_id}"))
            .header("Authorization", &self.auth)
            .json(&json!({ "content": content }))
            .send()
            .await?;
        parse(resp).await
    }

    /// Deletes a message (own messages, or any message with Manage Messages).
    pub async fn delete(&self, channel_id: &str, message_id: &str) -> Result<()> {
        let resp = self
            .http
            .delete(format!("{API}/channels/{channel_id}/messages/{message_id}"))
            .header("Authorization", &self.auth)
            .send()
            .await?;
        check(resp).await?;
        Ok(())
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let resp = self
            .http
            .get(format!("{API}{path}"))
            .header("Authorization", &self.auth)
            .send()
            .await?;
        parse(resp).await
    }
}

async fn check(resp: Response) -> Result<Response> {
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("Discord API returned {status}: {body}");
    }
    Ok(resp)
}

async fn parse<T: DeserializeOwned>(resp: Response) -> Result<T> {
    let resp = check(resp).await?;
    Ok(resp.json::<T>().await?)
}
