//! 离线 fixture：把真实抓取到的 JSON 存成文件，用于开发、演示、回归测试。
//!
//! 目录形式（推荐）：
//!
//! ```text
//! fixtures/steam/
//!   wishlist.json    # { "steam_id": "...", "wishlist": { "<appid>": { ...wishlistdata... } } }
//!   library.json     # GetOwnedGames 形态：{ "response": { "game_count": n, "games": [...] } }（可选）
//!   appdetails.json  # { "<appid>": { "success": true, "data": { ...appdetails... } } }
//! ```
//!
//! 也可以直接指向单个 `wishlist.json`（同目录下的 `appdetails.json` / `library.json`
//! 会被自动发现）。`library.json` 缺失时库存导入会明确报错，而不是静默返回空库存。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acgn_core::{Error, Result};

use crate::appdetails::AppDetailsResponse;
use crate::library::OwnedGame;
use crate::wishlist::WishlistItem;

/// 载入后的 fixture 数据。
#[derive(Debug, Clone)]
pub struct FixtureData {
    pub steam_id: Option<String>,
    pub wishlist: BTreeMap<String, WishlistItem>,
    /// 已拥有游戏（GetOwnedGames 形态；文件缺失时为空）
    pub library: Vec<OwnedGame>,
    pub appdetails: BTreeMap<String, AppDetailsResponse>,
    pub wishlist_path: PathBuf,
    pub library_path: Option<PathBuf>,
    pub appdetails_path: Option<PathBuf>,
}

impl FixtureData {
    /// 从目录或单个文件载入。
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let (wishlist_path, library_path, appdetails_path) = resolve_paths(path)?;

        let raw = std::fs::read_to_string(&wishlist_path).map_err(Error::storage)?;
        let value: serde_json::Value = serde_json::from_str(&raw).map_err(|err| {
            Error::invalid(format!(
                "fixture 不是合法 JSON（{}）: {err}",
                wishlist_path.display()
            ))
        })?;

        let (steam_id, wishlist_value) = match value.get("wishlist").or_else(|| value.get("items"))
        {
            Some(inner) => (
                value
                    .get("steam_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                inner.clone(),
            ),
            None => (None, value.clone()),
        };

        let wishlist: BTreeMap<String, WishlistItem> = serde_json::from_value(wishlist_value)
            .map_err(|err| {
                Error::invalid(format!(
                    "fixture 愿望单结构不对（{}）: {err}",
                    wishlist_path.display()
                ))
            })?;

        let appdetails = match appdetails_path {
            Some(ref appdetails_path) if appdetails_path.exists() => {
                let raw = std::fs::read_to_string(appdetails_path).map_err(Error::storage)?;
                let value: serde_json::Value = serde_json::from_str(&raw).map_err(|err| {
                    Error::invalid(format!("appdetails fixture 不是合法 JSON: {err}"))
                })?;
                let map = match value.get("appdetails").or_else(|| value.get("items")) {
                    Some(inner) => inner.clone(),
                    None => value,
                };

                let mut parsed: BTreeMap<String, AppDetailsResponse> = BTreeMap::new();
                if let Some(object) = map.as_object() {
                    for (appid, entry) in object {
                        let mut response: AppDetailsResponse =
                            serde_json::from_value(entry.clone()).map_err(|err| {
                                Error::invalid(format!(
                                    "appdetails fixture 结构不对（{appid}）: {err}"
                                ))
                            })?;
                        // 与 live 路径（`parse_app_details`）一致：把 data 对象原样存进 raw。
                        // 否则 fixture 模式下补全出来的条目 `metadata` 恒为 None，会掩盖
                        // 「导入 → 补全 → 再导入」这类只有真实网络才暴露的问题。
                        if let Some(data) = response.data.as_mut() {
                            data.raw = entry.get("data").cloned();
                        }
                        parsed.insert(appid.clone(), response);
                    }
                }
                parsed
            }
            _ => BTreeMap::new(),
        };

        let library = match library_path {
            Some(ref library_path) if library_path.exists() => load_library(library_path)?,
            _ => Vec::new(),
        };

        Ok(Self {
            steam_id,
            wishlist,
            library,
            appdetails,
            wishlist_path,
            library_path,
            appdetails_path,
        })
    }

    /// `(appid, item)` 列表。
    pub fn wishlist_entries(&self) -> Vec<(String, WishlistItem)> {
        self.wishlist
            .iter()
            .map(|(appid, item)| (appid.clone(), item.clone()))
            .collect()
    }

    /// 已拥有游戏列表。
    pub fn library_games(&self) -> Vec<OwnedGame> {
        self.library.clone()
    }

    pub fn app_details(&self, appid: &str) -> Option<&AppDetailsResponse> {
        self.appdetails.get(appid)
    }
}

/// 载入 `library.json`：兼容 GetOwnedGames 原始响应、简化对象与裸数组三种形态。
fn load_library(path: &Path) -> Result<Vec<OwnedGame>> {
    let raw = std::fs::read_to_string(path).map_err(Error::storage)?;
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|err| {
        Error::invalid(format!(
            "library fixture 不是合法 JSON（{}）: {err}",
            path.display()
        ))
    })?;

    let games_value = value
        .get("response")
        .and_then(|response| response.get("games"))
        .or_else(|| value.get("games"))
        .or_else(|| value.get("library"))
        .or_else(|| value.get("items"))
        .cloned()
        .unwrap_or(value);

    serde_json::from_value(games_value).map_err(|err| {
        Error::invalid(format!(
            "library fixture 结构不对（{}）: {err}",
            path.display()
        ))
    })
}

fn resolve_paths(path: &Path) -> Result<(PathBuf, Option<PathBuf>, Option<PathBuf>)> {
    if path.is_dir() {
        let wishlist = path.join("wishlist.json");
        if !wishlist.exists() {
            return Err(Error::not_found(format!(
                "fixture 目录里找不到 wishlist.json：{}",
                path.display()
            )));
        }
        return Ok((
            wishlist,
            Some(path.join("library.json")),
            Some(path.join("appdetails.json")),
        ));
    }

    if path.is_file() {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        return Ok((
            path.to_path_buf(),
            Some(parent.join("library.json")),
            Some(parent.join("appdetails.json")),
        ));
    }

    Err(Error::not_found(format!(
        "fixture 路径不存在：{}",
        path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use acgn_core::MetadataLevel;

    /// fixture 路径必须像 live 一样保留原始 data 载荷，否则补全出来的条目
    /// `metadata` 恒为 None，会掩盖「导入 → 补全 → 再导入」这类问题。
    #[test]
    fn restores_raw_payload_from_fixture() {
        let dir = std::env::temp_dir().join(format!("acgn-fixture-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        std::fs::write(
            dir.join("wishlist.json"),
            r#"{"steam_id":"76561198000000000","wishlist":{"1":{"name":"A","type":"game"}}}"#,
        )
        .expect("写 wishlist fixture");
        std::fs::write(
            dir.join("appdetails.json"),
            r#"{"1":{"success":true,"data":{"name":"A","type":"game"}}}"#,
        )
        .expect("写 appdetails fixture");
        std::fs::write(
            dir.join("library.json"),
            r#"{"steam_id":"76561198000000000","response":{"game_count":1,"games":[{"appid":1,"name":"A","playtime_forever":123}]}}"#,
        )
        .expect("写 library fixture");

        let fixture = FixtureData::load(&dir).expect("fixture 应能载入");
        assert_eq!(fixture.steam_id.as_deref(), Some("76561198000000000"));

        let response = fixture.app_details("1").expect("应有详情");
        let data = response.data.as_ref().expect("应有 data");
        assert!(data.raw.is_some(), "fixture 也应保留原始 data 载荷");

        let item = data.to_source_item("1");
        assert!(item.metadata.is_some(), "metadata 不能是 None");
        assert_eq!(item.metadata_level, MetadataLevel::Full);

        let games = fixture.library_games();
        assert_eq!(games.len(), 1, "库存 fixture 应被载入");
        assert_eq!(games[0].appid, 1);
        assert_eq!(games[0].playtime_forever, Some(123));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 缺少 `library.json` 时库存为空（但要能载入 wishlist fixture）：
    /// 库存导入会据此给出明确错误，而不是静默返回空库存。
    #[test]
    fn missing_library_fixture_is_not_an_error() {
        let dir = std::env::temp_dir().join(format!("acgn-fixture-nolib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        std::fs::write(
            dir.join("wishlist.json"),
            r#"{"wishlist":{"1":{"name":"A"}}}"#,
        )
        .expect("写 wishlist fixture");

        let fixture = FixtureData::load(&dir).expect("fixture 应能载入");
        assert!(fixture.library_games().is_empty());
        assert!(
            fixture.library_path.is_some(),
            "路径仍应指向预期的 library.json"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
