//! Checks GitHub for a newer Rustcord release and downloads the installer for this platform.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

const RELEASES_API: &str = "https://api.github.com/repos/KMThegamer2022/rustcord/releases/latest";

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: String,
    pub download_url: String,
    pub file_name: String,
}

/// Installer file name suffix for this platform.
fn platform_suffix() -> &'static str {
    if cfg!(target_os = "macos") {
        "-macos.dmg"
    } else {
        "-windows-setup.exe"
    }
}

/// Parses "v1.2.3" or "1.2.3" into numbers for comparison.
fn parse_version(v: &str) -> Vec<u64> {
    v.trim_start_matches('v')
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    let (a, b) = (parse_version(latest), parse_version(current));
    let len = a.len().max(b.len());
    for i in 0..len {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .user_agent("Rustcord-updater/0.1")
        .timeout(Duration::from_secs(60))
        .build()
        .expect("failed to build HTTP client")
}

/// Looks for a newer release. Returns None when up to date or when the check fails.
pub fn check() -> Option<UpdateInfo> {
    let release: serde_json::Value = client()
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .ok()?;

    let tag = release["tag_name"].as_str()?;
    if !is_newer(tag, env!("CARGO_PKG_VERSION")) {
        return None;
    }
    let suffix = platform_suffix();
    let asset = release["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(suffix)))?;

    Some(UpdateInfo {
        version: tag.trim_start_matches('v').to_string(),
        download_url: asset["browser_download_url"].as_str()?.to_string(),
        file_name: asset["name"].as_str()?.to_string(),
    })
}

/// Starts the check on a background thread and sends the result (if any) back.
pub fn check_in_background(tx: Sender<UpdateInfo>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        if let Some(info) = check() {
            let _ = tx.send(info);
            ctx.request_repaint();
        }
    });
}

/// Downloads the installer into the Downloads folder and opens it.
pub fn download_and_open(info: &UpdateInfo) -> Result<PathBuf, String> {
    let dir = dirs_downloads().ok_or("could not find the Downloads folder")?;
    let path = dir.join(&info.file_name);
    let bytes = client()
        .get(&info.download_url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| format!("download failed: {e}"))?;
    std::fs::write(&path, &bytes).map_err(|e| format!("could not save the installer: {e}"))?;

    let opened = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(&path).spawn()
    } else {
        std::process::Command::new("cmd").args(["/C", "start", ""]).arg(&path).spawn()
    };
    opened.map_err(|e| format!("could not open the installer: {e}"))?;
    Ok(path)
}

fn dirs_downloads() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let dir = PathBuf::from(home).join("Downloads");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }
}
