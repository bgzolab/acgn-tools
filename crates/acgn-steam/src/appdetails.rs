//! Steam 商店详情（appdetails）解析与转换。
//!
//! `store.steampowered.com/api/appdetails?appids=<id>&cc=cn&l=schinese`
//! 返回 `{ "<appid>": { "success": true, "data": { ... } } }`。
//! 这是愿望单接口之外的「补全」数据：简介、开发商、价格、更精确的发售日、平台支持。

use acgn_core::{MediaKind, MetadataLevel, Source, SourceItem};
use serde::{Deserialize, Serialize};

use crate::util::parse_release_date;
use crate::{header_url, poster_url};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppDetailsResponse {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub data: Option<AppDetailsData>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppDetailsData {
    #[serde(default, rename = "type")]
    pub app_type: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub steam_appid: Option<u64>,
    #[serde(default)]
    pub is_free: Option<bool>,
    #[serde(default)]
    pub short_description: Option<String>,
    #[serde(default)]
    pub detailed_description: Option<String>,
    #[serde(default)]
    pub header_image: Option<String>,
    #[serde(default)]
    pub capsule_image: Option<String>,
    #[serde(default)]
    pub capsule_imagev5: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub developers: Option<Vec<String>>,
    #[serde(default)]
    pub publishers: Option<Vec<String>>,
    #[serde(default)]
    pub genres: Option<Vec<NamedEntry>>,
    #[serde(default)]
    pub categories: Option<Vec<NamedEntry>>,
    #[serde(default)]
    pub platforms: Option<Platforms>,
    #[serde(default)]
    pub price_overview: Option<PriceOverview>,
    #[serde(default)]
    pub release_date: Option<ReleaseDate>,
    #[serde(default)]
    pub metacritic: Option<Metacritic>,
    /// 原始 data 对象，落库到 `games.metadata_json`
    #[serde(skip)]
    pub raw: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NamedEntry {
    /// Steam 的 genres 用字符串 id（"1"），categories 用数字 id（2），这里两种都收。
    #[serde(default, deserialize_with = "string_or_number")]
    pub id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// 把字符串或数字统一读成 `Option<String>`。
fn string_or_number<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.map(|value| match value {
        serde_json::Value::String(text) => text,
        other => other.to_string(),
    }))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Platforms {
    #[serde(default)]
    pub windows: bool,
    #[serde(default)]
    pub mac: bool,
    #[serde(default)]
    pub linux: bool,
}

impl Platforms {
    pub fn to_list(&self) -> Vec<String> {
        let mut list = Vec::new();
        if self.windows {
            list.push("windows".to_string());
        }
        if self.mac {
            list.push("mac".to_string());
        }
        if self.linux {
            list.push("linux".to_string());
        }
        list
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PriceOverview {
    #[serde(default)]
    pub currency: Option<String>,
    /// 单位：分
    #[serde(default)]
    pub initial: Option<i64>,
    #[serde(default, rename = "final")]
    pub final_price: Option<i64>,
    #[serde(default)]
    pub discount_percent: Option<i64>,
    #[serde(default)]
    pub initial_formatted: Option<String>,
    #[serde(default)]
    pub final_formatted: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReleaseDate {
    #[serde(default)]
    pub coming_soon: bool,
    #[serde(default)]
    pub date: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metacritic {
    #[serde(default)]
    pub score: Option<i64>,
    #[serde(default)]
    pub url: Option<String>,
}

impl AppDetailsData {
    pub fn media_kind(&self) -> MediaKind {
        self.app_type
            .as_deref()
            .map(MediaKind::from_steam_type)
            .unwrap_or(MediaKind::Game)
    }

    pub fn developers(&self) -> Vec<String> {
        self.developers.clone().unwrap_or_default()
    }

    pub fn publishers(&self) -> Vec<String> {
        self.publishers.clone().unwrap_or_default()
    }

    pub fn genres(&self) -> Vec<String> {
        self.genres
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| entry.description.clone())
            .collect()
    }

    fn release_raw(&self) -> Option<String> {
        self.release_date
            .as_ref()
            .map(|release| release.date.trim().to_string())
            .filter(|date| !date.is_empty())
    }

    /// 转换为统一 [`SourceItem`]（不含个人状态，避免覆盖用户数据）。
    ///
    /// 标题处理：`l=schinese` 时 Steam 会返回中文名（如「艾尔登法环」）。
    /// 中文名进 `title_zh`，`title` 留空，这样合并时不会把愿望单里的英文原名冲掉，
    /// 前端展示则优先用 `title_zh`。
    pub fn to_source_item(&self, appid: &str) -> SourceItem {
        let release_raw = self.release_raw();
        let name = self.name.clone().unwrap_or_default().trim().to_string();
        let (title, title_zh) = if has_cjk(&name) {
            (String::new(), Some(name))
        } else {
            (name, None)
        };

        SourceItem {
            source: Source::Steam,
            external_id: appid.to_string(),
            media_kind: self.media_kind(),
            title,
            title_zh,
            short_description: self.short_description.clone(),
            cover_url: self
                .header_image
                .clone()
                .or_else(|| self.capsule_imagev5.clone())
                .or_else(|| self.capsule_image.clone())
                .or_else(|| Some(header_url(appid))),
            poster_url: Some(poster_url(appid)),
            release_date: release_raw.as_deref().and_then(parse_release_date),
            release_raw,
            review_score: None,
            review_desc: None,
            review_count: None,
            is_free: self.is_free,
            price_currency: self
                .price_overview
                .as_ref()
                .and_then(|price| price.currency.clone()),
            price_initial: self.price_overview.as_ref().and_then(|price| price.initial),
            price_final: self
                .price_overview
                .as_ref()
                .and_then(|price| price.final_price),
            discount_percent: self
                .price_overview
                .as_ref()
                .and_then(|price| price.discount_percent),
            developers: self.developers(),
            publishers: self.publishers(),
            genres: self.genres(),
            platforms: self
                .platforms
                .as_ref()
                .map(Platforms::to_list)
                .unwrap_or_default(),
            tags: Vec::new(),
            status: None,
            priority: None,
            source_added_at: None,
            // appdetails 不携带游玩时长（时长只在库存接口里）：补全时不能覆盖时长跟踪值
            playtime_minutes: None,
            playtime_2weeks_minutes: None,
            last_played_at: None,
            metadata: self.raw.clone(),
            metadata_level: MetadataLevel::Full,
        }
    }
}

/// 解析 appdetails 响应里某个 appid 的条目。
pub fn parse_app_details(raw: &serde_json::Value) -> Option<AppDetailsData> {
    let response: AppDetailsResponse = serde_json::from_value(raw.clone()).ok()?;
    if !response.success {
        return None;
    }
    let mut data = response.data?;
    data.raw = raw.get("data").cloned();
    Some(data)
}

/// 是否包含 CJK 字符（用来判断 Steam 返回的名字是不是中文名）。
pub fn has_cjk(value: &str) -> bool {
    value.chars().any(|c| {
        matches!(c as u32,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2A6DF)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "success": true,
        "data": {
            "type": "game",
            "name": "ELDEN RING",
            "is_free": false,
            "short_description": "Rise, Tarnished.",
            "header_image": "https://example.com/header.jpg",
            "developers": ["FromSoftware, Inc."],
            "publishers": ["Bandai Namco Entertainment"],
            "genres": [{"id": "1", "description": "Action"}],
            "platforms": {"windows": true, "mac": false, "linux": false},
            "price_overview": {"currency": "CNY", "initial": 29800, "final": 19900, "discount_percent": 33},
            "release_date": {"coming_soon": false, "date": "25 Feb, 2022"}
        }
    }"#;

    #[test]
    fn converts_app_details() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let data = parse_app_details(&value).expect("should parse");
        let item = data.to_source_item("1245620");

        assert_eq!(item.title, "ELDEN RING");
        assert!(item.title_zh.is_none());
        assert_eq!(item.developers, vec!["FromSoftware, Inc."]);
        assert_eq!(item.genres, vec!["Action"]);
        assert_eq!(item.platforms, vec!["windows"]);
        assert_eq!(item.price_currency.as_deref(), Some("CNY"));
        assert_eq!(item.price_final, Some(19900));
        assert_eq!(item.discount_percent, Some(33));
        assert_eq!(item.release_date.as_deref(), Some("2022-02-25"));
        assert!(item.metadata.is_some());
    }

    #[test]
    fn keeps_chinese_name_as_title_zh() {
        let raw = r#"{"success": true, "data": {"name": "艾尔登法环", "type": "game"}}"#;
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        let item = parse_app_details(&value).unwrap().to_source_item("1245620");
        assert_eq!(item.title, "");
        assert_eq!(item.title_zh.as_deref(), Some("艾尔登法环"));
    }

    #[test]
    fn handles_unsuccessful_response() {
        let value: serde_json::Value = serde_json::from_str(r#"{"success": false}"#).unwrap();
        assert!(parse_app_details(&value).is_none());
    }

    #[test]
    fn tolerates_numeric_category_ids() {
        // Steam 的 categories 用数字 id，genres 用字符串 id，两种都不能炸
        let raw = r#"{"success": true, "data": {
            "name": "X",
            "genres": [{"id": "1", "description": "动作"}],
            "categories": [{"id": 2, "description": "多人"}]
        }}"#;
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        let data = parse_app_details(&value).expect("应能解析");
        assert_eq!(data.genres().len(), 1);
        assert_eq!(
            data.categories.as_ref().unwrap()[0].id.as_deref(),
            Some("2")
        );
    }
}
