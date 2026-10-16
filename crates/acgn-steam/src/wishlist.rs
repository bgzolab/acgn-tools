//! Steam 愿望单载荷解析。
//!
//! 公开接口 `store.steampowered.com/wishlist/profiles/<id>/wishlistdata/?p=N` 返回
//! `{ "<appid>": { ... } }`；老版本也可能返回数组，这里两种都兼容。
//! 所有字段都是宽松解析：Steam 随时可能增减字段，缺字段不能让导入整体失败。

use std::collections::BTreeMap;

use acgn_core::{MediaKind, MetadataLevel, Source, SourceItem};
use serde::{Deserialize, Serialize};

use crate::util::{from_unix, parse_count, parse_release_date};
use crate::{header_url, poster_url};

/// 单条愿望单数据。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WishlistItem {
    #[serde(default)]
    pub name: Option<String>,
    /// 新接口里的封面 hash（拼接方式随版本变化，默认用 CDN 固定路径）
    #[serde(default)]
    pub capsule: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub review_score: Option<i64>,
    #[serde(default)]
    pub review_desc: Option<String>,
    /// "1,234" 这样的字符串
    #[serde(default)]
    pub reviews_total: Option<String>,
    #[serde(default)]
    pub reviews_percent: Option<i64>,
    #[serde(default)]
    pub release_string: Option<String>,
    #[serde(default)]
    pub release_date: Option<i64>,
    /// 条目类型，通常叫 `type`
    #[serde(default, rename = "type")]
    pub entry_type: Option<String>,
    /// 有的载荷用 `app_type`
    #[serde(default)]
    pub app_type: Option<String>,
    /// 用户在愿望单里给它的排位（0 最靠前）
    #[serde(default)]
    pub priority: Option<i64>,
    #[serde(default)]
    pub rank: Option<i64>,
    /// 加入愿望单的 Unix 秒
    #[serde(default)]
    pub added: Option<i64>,
    #[serde(default)]
    pub is_free_game: Option<bool>,
    #[serde(default)]
    pub early_access: Option<bool>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub store_url: Option<String>,
}

impl WishlistItem {
    pub fn media_kind(&self) -> MediaKind {
        self.entry_type
            .as_deref()
            .or(self.app_type.as_deref())
            .map(MediaKind::from_steam_type)
            .unwrap_or(MediaKind::Game)
    }

    /// 从 `store_url` 里抠 appid（数组格式载荷的兜底）。
    ///
    /// 兼容 `https://store.steampowered.com/app/1245620/ELDEN_RING/`、
    /// `steam://store/570` 等写法：取 URL 里最后一段连续数字。
    pub fn appid_from_store_url(&self) -> Option<String> {
        let url = self.store_url.as_deref()?;
        let digits: String = url
            .rsplit(|c: char| !c.is_ascii_digit())
            .find(|part| !part.is_empty() && part.len() <= 10)?
            .to_string();
        (!digits.is_empty()).then_some(digits)
    }
}

/// 愿望单响应：对象映射或数组都支持。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WishlistPayload {
    Map(BTreeMap<String, WishlistItem>),
    List(Vec<WishlistItem>),
}

impl WishlistPayload {
    pub fn is_empty(&self) -> bool {
        match self {
            WishlistPayload::Map(map) => map.is_empty(),
            WishlistPayload::List(list) => list.is_empty(),
        }
    }

    /// 展开成 `(appid, item)`；数组格式尽量从 `store_url` 里恢复 appid。
    pub fn into_entries(self) -> Vec<(Option<String>, WishlistItem)> {
        match self {
            WishlistPayload::Map(map) => {
                map.into_iter().map(|(id, item)| (Some(id), item)).collect()
            }
            WishlistPayload::List(list) => list
                .into_iter()
                .map(|item| (item.appid_from_store_url(), item))
                .collect(),
        }
    }
}

/// 愿望单条目 → 统一 [`SourceItem`]。
pub fn wishlist_to_source_item(appid: &str, item: &WishlistItem) -> SourceItem {
    let release_raw = item
        .release_string
        .clone()
        .filter(|raw| !raw.trim().is_empty());

    SourceItem {
        source: Source::Steam,
        external_id: appid.to_string(),
        media_kind: item.media_kind(),
        title: item.name.clone().unwrap_or_default(),
        title_zh: None,
        short_description: None,
        cover_url: Some(header_url(appid)),
        poster_url: Some(poster_url(appid)),
        release_date: release_raw.as_deref().and_then(parse_release_date),
        release_raw,
        review_score: item.review_score,
        review_desc: item.review_desc.clone(),
        review_count: item.reviews_total.as_deref().and_then(parse_count),
        is_free: item.is_free_game,
        // 愿望单接口不带价格，价格交给 appdetails 补全
        price_currency: None,
        price_initial: None,
        price_final: None,
        discount_percent: None,
        developers: Vec::new(),
        publishers: Vec::new(),
        genres: Vec::new(),
        platforms: Vec::new(),
        tags: item.tags.clone().unwrap_or_default(),
        status: None,
        priority: item.priority.or(item.rank),
        source_added_at: from_unix(item.added),
        metadata: serde_json::to_value(item).ok(),
        // 愿望单接口只有通用封面 URL，属于「列表档」
        metadata_level: MetadataLevel::List,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "1245620": {
            "name": "ELDEN RING",
            "capsule": "abc?t=1",
            "review_score": 8,
            "review_desc": "Very Positive",
            "reviews_total": "1,234",
            "release_string": "25 Feb, 2022",
            "type": "game",
            "priority": 0,
            "added": 1718000000,
            "is_free_game": false,
            "tags": ["Souls-like", "RPG"]
        },
        "2358720": {
            "name": "Black Myth: Wukong",
            "review_score": 8,
            "release_string": "19 Aug, 2024",
            "type": "game",
            "priority": 1,
            "added": 1720000000
        }
    }"#;

    #[test]
    fn parses_map_payload() {
        let payload: WishlistPayload = serde_json::from_str(SAMPLE).unwrap();
        assert!(!payload.is_empty());
        let entries = payload.into_entries();
        assert_eq!(entries.len(), 2);

        let item = wishlist_to_source_item("1245620", &entries[0].1);
        assert_eq!(item.title, "ELDEN RING");
        assert_eq!(item.external_id, "1245620");
        assert_eq!(item.review_count, Some(1234));
        assert_eq!(item.release_date.as_deref(), Some("2022-02-25"));
        assert_eq!(item.priority, Some(0));
        assert!(item.source_added_at.is_some());
        assert!(item.cover_url.unwrap().ends_with("1245620/header.jpg"));
    }

    #[test]
    fn tolerates_empty_object() {
        let payload: WishlistPayload = serde_json::from_str("{}").unwrap();
        assert!(payload.is_empty());
    }

    #[test]
    fn tolerates_array_payload() {
        let raw = r#"[{"name":"A","store_url":"steam://store/570"}]"#;
        let payload: WishlistPayload = serde_json::from_str(raw).unwrap();
        let entries = payload.into_entries();
        assert_eq!(entries[0].0.as_deref(), Some("570"));

        let raw = r#"[{"name":"B","store_url":"https://store.steampowered.com/app/1245620/ELDEN_RING/"}]"#;
        let payload: WishlistPayload = serde_json::from_str(raw).unwrap();
        let entries = payload.into_entries();
        assert_eq!(entries[0].0.as_deref(), Some("1245620"));
    }
}
