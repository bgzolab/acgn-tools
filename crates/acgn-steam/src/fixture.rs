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
                serde_json::from_value(map)
                    .map_err(|err| Error::invalid(format!("appdetails fixture 结构不对: {err}")))?
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
