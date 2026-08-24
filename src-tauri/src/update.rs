//! 对照 GitHub Release 看有没有新版本。
//!
//! 不做自动下载、不装 updater 插件：官网和 Release 资产已经是分发入口，
//! 这里只回答「当前是不是旧的」和「去哪下」。CSP 不许前端直连
//! api.github.com，所以检查必须走宿主。

use serde::{Deserialize, Serialize};

use crate::{HostError, HostResult};

const RELEASES_LATEST: &str = "https://api.github.com/repos/caiwuu/Riot/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/caiwuu/Riot/releases";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: String,
    pub latest: Option<String>,
    pub notes: Option<String>,
    /// 优先给当前平台的安装包，没有就给 Release 页。
    pub url: String,
    pub newer: bool,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

pub async fn check(current: &str) -> HostResult<UpdateInfo> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .user_agent(format!("Riot/{current} (update-check)"))
        .build()
        .map_err(|e| HostError::Update(format!("检查更新失败：{e}")))?;

    let res = client
        .get(RELEASES_LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| HostError::Update(format!("检查更新失败：{e}")))?;

    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(UpdateInfo {
            current: current.to_owned(),
            latest: None,
            notes: None,
            url: RELEASES_PAGE.into(),
            newer: false,
        });
    }
    if !res.status().is_success() {
        return Err(HostError::Update(format!(
            "检查更新失败：GitHub 返回 {}",
            res.status()
        )));
    }

    let body = res
        .text()
        .await
        .map_err(|e| HostError::Update(format!("检查更新失败：{e}")))?;
    let rel: GhRelease = serde_json::from_str(&body)
        .map_err(|e| HostError::Update(format!("检查更新失败：读不懂 GitHub 的回复（{e}）")))?;

    Ok(from_release(current, &rel))
}

fn from_release(current: &str, rel: &GhRelease) -> UpdateInfo {
    let latest = normalize_version(&rel.tag_name);
    let url = pick_asset(&rel.assets)
        .map(|a| a.browser_download_url.clone())
        .unwrap_or_else(|| rel.html_url.clone());
    let notes = rel
        .body
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    UpdateInfo {
        newer: is_newer(&latest, current),
        current: current.to_owned(),
        latest: Some(latest),
        notes,
        url,
    }
}

/// `Riot_0.1.0` / `v0.1.0` / `0.1.0` → `0.1.0`
pub fn normalize_version(raw: &str) -> String {
    let s = raw.trim();
    let s = s.strip_prefix("Riot_").unwrap_or(s);
    let s = s
        .strip_prefix('v')
        .or_else(|| s.strip_prefix('V'))
        .unwrap_or(s);
    s.trim().to_owned()
}

fn version_key(raw: &str) -> Option<Vec<u64>> {
    let s = normalize_version(raw);
    let mut parts = Vec::new();
    for p in s.split('.') {
        let digits = p
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>();
        if digits.is_empty() {
            return None;
        }
        parts.push(digits.parse().ok()?);
        if parts.len() == 3 {
            break;
        }
    }
    (!parts.is_empty()).then_some(parts)
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (version_key(latest), version_key(current)) {
        (Some(a), Some(b)) => a > b,
        _ => normalize_version(latest) != normalize_version(current),
    }
}

fn pick_asset(assets: &[GhAsset]) -> Option<&GhAsset> {
    let prefer: &[&str] = if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            &["aarch64.dmg", ".dmg"]
        } else {
            &["x64.dmg", ".dmg"]
        }
    } else if cfg!(target_os = "windows") {
        &["x64-setup.exe", "-setup.exe", ".exe"]
    } else {
        &[]
    };
    for suffix in prefer {
        if let Some(a) = assets.iter().find(|a| a.name.ends_with(suffix)) {
            return Some(a);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_都能收到三段版本号() {
        assert_eq!(normalize_version("Riot_0.1.0"), "0.1.0");
        assert_eq!(normalize_version("v0.2.1"), "0.2.1");
        assert_eq!(normalize_version("0.1.0"), "0.1.0");
    }

    #[test]
    fn 新版本比当前高才算有更新() {
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("Riot_0.2.0", "0.1.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.1"));
    }

    #[test]
    fn 没发布过就当没有更新() {
        let info = from_release(
            "0.1.0",
            &GhRelease {
                tag_name: "Riot_0.1.0".into(),
                html_url: RELEASES_PAGE.into(),
                body: None,
                assets: vec![],
            },
        );
        assert!(!info.newer);
        assert_eq!(info.latest.as_deref(), Some("0.1.0"));
    }

    #[test]
    fn 有更高的tag就算有更新() {
        let info = from_release(
            "0.1.0",
            &GhRelease {
                tag_name: "Riot_0.1.1".into(),
                html_url: "https://github.com/caiwuu/Riot/releases/tag/Riot_0.1.1".into(),
                body: Some("修复".into()),
                assets: vec![GhAsset {
                    name: "Riot_0.1.1_aarch64.dmg".into(),
                    browser_download_url: "https://example.test/Riot.dmg".into(),
                }],
            },
        );
        assert!(info.newer);
        assert_eq!(info.latest.as_deref(), Some("0.1.1"));
        assert_eq!(info.notes.as_deref(), Some("修复"));
    }
}
