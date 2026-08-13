//! Channel-aware app update check (stable vs beta GitHub feeds).
//!
//! The JS updater plugin cannot override endpoints, so the host builds the
//! updater with the feed for [`boris_pipeline::AppSettings::update_channel`]
//! and registers the resulting `Update` resource so download/install still
//! go through `tauri-plugin-updater`.

use serde::Serialize;
use tauri::{Manager, ResourceId, Webview};
use tauri_plugin_updater::UpdaterExt;
use url::Url;

/// GitHub `/releases/latest` — never includes pre-releases.
pub const STABLE_ENDPOINT: &str =
    "https://github.com/blocksdevpro/boris-assistant/releases/latest/download/latest.json";

/// Long-lived pre-release tag `beta` (overwrite assets on each beta).
pub const BETA_ENDPOINT: &str =
    "https://github.com/blocksdevpro/boris-assistant/releases/download/beta/latest.json";

pub fn endpoint_for_channel(channel: &str) -> &'static str {
    if channel.eq_ignore_ascii_case("beta") {
        BETA_ENDPOINT
    } else {
        STABLE_ENDPOINT
    }
}

/// Same camelCase shape the JS `Update` constructor expects.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckMeta {
    rid: ResourceId,
    current_version: String,
    version: String,
    date: Option<String>,
    body: Option<String>,
    raw_json: serde_json::Value,
}

/// Check the stable or beta `latest.json` feed.
#[tauri::command]
pub async fn check_app_update(
    webview: Webview,
    channel: Option<String>,
) -> Result<Option<UpdateCheckMeta>, String> {
    let channel = channel.unwrap_or_else(|| "stable".into());
    let endpoint = endpoint_for_channel(&channel);
    tracing::info!(%channel, %endpoint, "check_app_update");

    let url = Url::parse(endpoint).map_err(|e| format!("invalid updater endpoint: {e}"))?;
    let updater = webview
        .updater_builder()
        .endpoints(vec![url])
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?;

    let update = match updater.check().await {
        Ok(found) => found,
        Err(e) => {
            let msg = e.to_string();
            if channel.eq_ignore_ascii_case("beta") && looks_like_missing_feed(&msg) {
                return Err("No beta builds published yet.".into());
            }
            return Err(msg);
        }
    };

    let Some(update) = update else {
        return Ok(None);
    };

    Ok(Some(UpdateCheckMeta {
        current_version: update.current_version.clone(),
        version: update.version.clone(),
        date: update.date.map(|d| d.to_string()),
        body: update.body.clone(),
        raw_json: update.raw_json.clone(),
        rid: webview.resources_table().add(update),
    }))
}

fn looks_like_missing_feed(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("404") || lower.contains("not found") || lower.contains("status code: 404")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beta_channel_uses_beta_tag_feed() {
        assert_eq!(endpoint_for_channel("beta"), BETA_ENDPOINT);
        assert_eq!(endpoint_for_channel("BETA"), BETA_ENDPOINT);
        assert_eq!(endpoint_for_channel("stable"), STABLE_ENDPOINT);
        assert_eq!(endpoint_for_channel(""), STABLE_ENDPOINT);
    }

    #[test]
    fn missing_feed_detects_github_404() {
        assert!(looks_like_missing_feed("failed to check for updates: 404 Not Found"));
        assert!(!looks_like_missing_feed("signature verification failed"));
    }
}
