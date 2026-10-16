//! 离线 fixture：把真实抓取到的 JSON 存成文件，用于开发、演示、回归测试。
//!
//! 目录形式（推荐）：
//!
//! ```text
//! fixtures/steam/
//!   wishlist.json    # { "steam_id": "...", "wishlist": { "<appid>": { ...wishlistdata... } } }
//!   appdetails.json  # { "<appid>": { "success": true, "data": { ...appdetails... } } }
//! ```
//!
//! 也可以直接指向单个 `wishlist.json`（同目录下的 `appdetails.json` 会被自动发现）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acgn_core::{Error, Result};

use crate::appdetails::AppDetailsResponse;
use crate::wishlist::WishlistItem;

/// 载入后的 fixture 数据。
#[derive(Debug, Clone)]
pub struct FixtureData {
    pub steam_id: Option<String>,
    pub wishlist: BTreeMap<String, WishlistItem>,
    pub appdetails: BTreeMap<String, AppDetailsResponse>,
    pub wishlist_path: PathBuf,
    pub appdetails_path: Option<PathBuf>,
}

impl FixtureData {
    /// 从目录或单个文件载入。
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let (wishlist_path, appdetails_path) = resolve_paths(path)?;

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

        Ok(Self {
            steam_id,
            wishlist,
            appdetails,
            wishlist_path,
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

    pub fn app_details(&self, appid: &str) -> Option<&AppDetailsResponse> {
        self.appdetails.get(appid)
    }
}

fn resolve_paths(path: &Path) -> Result<(PathBuf, Option<PathBuf>)> {
    if path.is_dir() {
        let wishlist = path.join("wishlist.json");
        if !wishlist.exists() {
            return Err(Error::not_found(format!(
                "fixture 目录里找不到 wishlist.json：{}",
                path.display()
            )));
        }
        return Ok((wishlist, Some(path.join("appdetails.json"))));
    }

    if path.is_file() {
        let appdetails = path
            .parent()
            .map(|parent| parent.join("appdetails.json"))
            .unwrap_or_else(|| PathBuf::from("appdetails.json"));
        return Ok((path.to_path_buf(), Some(appdetails)));
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

        let fixture = FixtureData::load(&dir).expect("fixture 应能载入");
        assert_eq!(fixture.steam_id.as_deref(), Some("76561198000000000"));

        let response = fixture.app_details("1").expect("应有详情");
        let data = response.data.as_ref().expect("应有 data");
        assert!(data.raw.is_some(), "fixture 也应保留原始 data 载荷");

        let item = data.to_source_item("1");
        assert!(item.metadata.is_some(), "metadata 不能是 None");
        assert_eq!(item.metadata_level, MetadataLevel::Full);

        std::fs::remove_dir_all(&dir).ok();
    }
}
