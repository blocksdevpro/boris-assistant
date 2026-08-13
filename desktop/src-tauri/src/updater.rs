//! Channel-aware app update check (stable vs beta GitHub feeds).
//!
//! The JS updater plugin cannot override endpoints, so the host builds the
//! updater with the feed for [`boris_pipeline::AppSettings::update_channel`]
//! and registers the resulting `Update` resource so download/install still
//! go through `tauri-plugin-updater`.
//!
//! GitHub's `/releases/download/...` URL is instant in a browser and slow in
//! Rust for the same reason most Windows + reqwest apps are: a **new**
//! `reqwest::Client` asks WinHTTP for the system proxy (WPAD/PAC) on every
//! host, then does a cold TLS handshake and follows the asset-CDN redirect.
//! The browser already has that PAC result and an HTTP/2 socket cached, so
//! it looks "instant". `tune_github_client` disables WPAD (`no_proxy` on
//! Windows) and caps the connect phase; we also peek the Releases API so a
//! current install never has to touch the CDN.

use std::time::Duration;

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

const GITHUB_API_REPO: &str = "https://api.github.com/repos/blocksdevpro/boris-assistant";

/// Plugin fetch hits the asset CDN — only used when an update looks available
/// (or the API peek failed). Long enough for GitHub's ~10s asset redirect.
const PLUGIN_CHECK_TIMEOUT: Duration = Duration::from_secs(25);
/// Releases API JSON only — no asset CDN.
const PEEK_TIMEOUT: Duration = Duration::from_secs(8);

pub fn endpoint_for_channel(channel: &str) -> &'static str {
    if channel.eq_ignore_ascii_case("beta") {
        BETA_ENDPOINT
    } else {
        STABLE_ENDPOINT
    }
}

pub fn github_release_api(channel: &str) -> String {
    if channel.eq_ignore_ascii_case("beta") {
        format!("{GITHUB_API_REPO}/releases/tags/beta")
    } else {
        format!("{GITHUB_API_REPO}/releases/latest")
    }
}

/// Strip a leading `v` so `v1.1.0-beta.2` matches `1.1.0-beta.2`.
pub fn normalize_version(raw: &str) -> &str {
    raw.trim().trim_start_matches(['v', 'V'])
}

pub fn same_version(a: &str, b: &str) -> bool {
    !a.is_empty() && normalize_version(a) == normalize_version(b)
}

/// Newest versioned release on `channel`, skipping the rolling `beta` tag.
/// GitHub returns newest-first.
pub fn version_from_release_list(channel: &str, releases: &[serde_json::Value]) -> Option<String> {
    let want_prerelease = channel.eq_ignore_ascii_case("beta");
    releases.iter().find_map(|release| {
        let tag = release.get("tag_name")?.as_str()?;
        if tag.eq_ignore_ascii_case("beta") {
            return None;
        }
        let prerelease = release
            .get("prerelease")
            .and_then(|p| p.as_bool())
            .unwrap_or(false);
        if prerelease != want_prerelease {
            return None;
        }
        let version = normalize_version(tag);
        (!version.is_empty()).then(|| version.to_string())
    })
}

fn humanize_check_error(channel: &str, msg: &str) -> String {
    if looks_like_timeout(msg) || looks_like_send_failure(msg) {
        return "Could not reach GitHub Releases. Try again in a moment.".into();
    }
    if channel.eq_ignore_ascii_case("beta") && looks_like_missing_feed(msg) {
        return "No beta builds published yet.".into();
    }
    msg.to_string()
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
    let current = webview.app_handle().package_info().version.to_string();
    tracing::info!(%channel, %endpoint, %current, "check_app_update");

    match peek_remote_version(&channel).await {
        Ok(remote) if same_version(&current, &remote) => {
            tracing::info!(%channel, %current, "check_app_update: already on latest (api peek)");
            return Ok(None);
        }
        Ok(remote) => {
            tracing::info!(%channel, %current, %remote, "check_app_update: newer feed, confirming");
        }
        Err(e) => {
            tracing::warn!(%channel, error = %e, "check_app_update: peek skipped, using plugin");
        }
    }

    let url = Url::parse(endpoint).map_err(|e| format!("invalid updater endpoint: {e}"))?;
    let updater = webview
        .updater_builder()
        .endpoints(vec![url])
        .map_err(|e| e.to_string())?
        .timeout(PLUGIN_CHECK_TIMEOUT)
        .header("Accept", "*/*")
        .map_err(|e| e.to_string())?
        .configure_client(tune_github_client)
        .build()
        .map_err(|e| e.to_string())?;

    let update = match updater.check().await {
        Ok(found) => found,
        Err(e) => {
            let msg = e.to_string();
            tracing::warn!(%channel, error = %msg, "check_app_update failed");
            return Err(humanize_check_error(&channel, &msg));
        }
    };

    let Some(update) = update else {
        tracing::info!(%channel, %current, "check_app_update: already on latest");
        return Ok(None);
    };

    tracing::info!(
        %channel,
        current = %update.current_version,
        version = %update.version,
        "check_app_update: update available"
    );

    Ok(Some(UpdateCheckMeta {
        current_version: update.current_version.clone(),
        version: update.version.clone(),
        date: update.date.map(|d| d.to_string()),
        body: update.body.clone(),
        raw_json: update.raw_json.clone(),
        rid: webview.resources_table().add(update),
    }))
}

/// Shared by the API peek and the plugin download client.
///
/// Windows: skip WinHTTP WPAD/PAC. That lookup is cached in the browser and
/// redone from scratch for every new reqwest client — often 5–20s per host,
/// which is the whole "browser is instant, Rust is stuck" gap.
fn tune_github_client(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    let builder = builder
        .user_agent("boris-desktop")
        .connect_timeout(Duration::from_secs(3))
        .tcp_nodelay(true);
    // Public GitHub HTTPS. WPAD on Windows is the slow part, not the URL.
    #[cfg(windows)]
    let builder = builder.no_proxy();
    builder
}

async fn peek_remote_version(channel: &str) -> Result<String, String> {
    let client = tune_github_client(reqwest::Client::builder().timeout(PEEK_TIMEOUT))
        .build()
        .map_err(|e| e.to_string())?;

    // JSON catalog only — do not download release assets.
    let releases: Vec<serde_json::Value> = client
        .get(format!("{GITHUB_API_REPO}/releases?per_page=20"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    version_from_release_list(channel, &releases)
        .ok_or_else(|| format!("no {channel} release on GitHub"))
}

fn looks_like_missing_feed(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("404") || lower.contains("not found") || lower.contains("status code: 404")
}

fn looks_like_timeout(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("timed out") || lower.contains("timeout")
}

fn looks_like_send_failure(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("error sending request") || lower.contains("connection reset")
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
    fn github_api_urls_match_channel() {
        assert_eq!(
            github_release_api("beta"),
            "https://api.github.com/repos/blocksdevpro/boris-assistant/releases/tags/beta"
        );
        assert_eq!(
            github_release_api("stable"),
            "https://api.github.com/repos/blocksdevpro/boris-assistant/releases/latest"
        );
    }

    #[test]
    fn version_compare_ignores_v_prefix() {
        assert!(same_version("1.1.0-beta.2", "v1.1.0-beta.2"));
        assert!(same_version("v1.0.0", "1.0.0"));
        assert!(!same_version("1.1.0-beta.1", "1.1.0-beta.2"));
        assert!(!same_version("", "1.0.0"));
    }

    #[test]
    fn version_from_release_list_skips_rolling_beta_tag() {
        let releases = vec![
            serde_json::json!({ "tag_name": "beta", "prerelease": true }),
            serde_json::json!({ "tag_name": "v1.1.0-beta.2", "prerelease": true }),
            serde_json::json!({ "tag_name": "v1.1.0-beta.1", "prerelease": true }),
            serde_json::json!({ "tag_name": "v1.0.0", "prerelease": false }),
        ];
        assert_eq!(
            version_from_release_list("beta", &releases).as_deref(),
            Some("1.1.0-beta.2")
        );
        assert_eq!(
            version_from_release_list("stable", &releases).as_deref(),
            Some("1.0.0")
        );
        assert!(version_from_release_list("beta", &[]).is_none());
    }

    #[test]
    fn missing_feed_detects_github_404() {
        assert!(looks_like_missing_feed("failed to check for updates: 404 Not Found"));
        assert!(!looks_like_missing_feed("signature verification failed"));
        assert!(looks_like_timeout("error sending request: timed out"));
        assert!(!looks_like_timeout("signature verification failed"));
        assert!(looks_like_send_failure(
            "error sending request for url (https://github.com/x/latest.json)"
        ));
        assert_eq!(
            humanize_check_error(
                "beta",
                "error sending request for url (https://github.com/x/latest.json)"
            ),
            "Could not reach GitHub Releases. Try again in a moment."
        );
    }
}
