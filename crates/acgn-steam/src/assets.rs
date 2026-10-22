//! 商店目录 / 资产（`IStoreBrowseService`）：名称、类型与竖版海报。
//!
//! `appdetails` 只给横版 `header.jpg`；竖版 `library_capsule`（600x900）要从
//! `api.steampowered.com/IStoreBrowseService/GetItems/v1` 取（匿名可用、支持批量）。
//! 同一个接口还带**本地化名称与类型**，所以官方愿望单（只返回 appid）可以用它批量
//! 补全标题，避免出现大量 `Steam App XXX` 占位符。
//!
//! 为什么不用旧式路径 `cdn.cloudflare.steamstatic.com/steam/apps/{id}/library_600x900.jpg`：
//! 实测它对一部分游戏直接 404（成人 / 区域受限内容为主），卡片会退化成文字占位。
//!
//! 注意：在国区上下文（`cc=cn`）下，这类游戏的 assets 会被**整体隐藏**（不是逐条 404），
//! 所以调用方拿不到时要换 `us` 再试——与 `appdetails` 的国家回退是同一套思路。

use std::collections::HashMap;

use acgn_core::{MediaKind, SourceItem};
use reqwest::Url;
use serde::Deserialize;

use crate::API_BASE;

/// 新式资产 CDN；与 `appdetails` 返回的 header 同源。
pub const ASSET_CDN_BASE: &str = "https://shared.akamai.steamstatic.com/store_item_assets";

/// 一次批量请求最多带多少个 appid（URL 长度保险）。
pub const STORE_ITEMS_CHUNK: usize = 50;

/// `IStoreBrowseService/GetItems/v1` 请求 URL。
///
/// `input_json` 走 percent-encoding：里面是带 `{}` / `"` 的 JSON，
/// 直接拼进 query 会被截断或转义出错。
pub fn store_items_url(appids: &[String], country: &str, language: &str) -> String {
    let ids: Vec<serde_json::Value> = appids
        .iter()
        .filter_map(|appid| appid.parse::<u64>().ok())
        .map(|appid| serde_json::json!({ "appid": appid }))
        .collect();

    let input = serde_json::json!({
        "ids": ids,
        "context": { "language": language, "country_code": country },
        "data_request": { "include_assets": true },
    })
    .to_string();

    let mut url = Url::parse(&format!("{API_BASE}/IStoreBrowseService/GetItems/v1/"))
        .expect("静态 API 地址应当合法");
    url.query_pairs_mut().append_pair("input_json", &input);
    url.to_string()
}

#[derive(Debug, Default, Deserialize)]
struct GetItemsResponse {
    #[serde(default)]
    response: GetItemsEnvelope,
}

#[derive(Debug, Default, Deserialize)]
struct GetItemsEnvelope {
    #[serde(default)]
    store_items: Vec<StoreItem>,
}

#[derive(Debug, Default, Deserialize)]
struct StoreItem {
    appid: Option<u64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "type")]
    store_type: Option<i64>,
    #[serde(default)]
    assets: ItemAssets,
}

#[derive(Debug, Default, Deserialize)]
struct ItemAssets {
    #[serde(default)]
    asset_url_format: Option<String>,
    #[serde(default)]
    library_capsule: Option<String>,
    #[serde(default)]
    library_capsule_2x: Option<String>,
}

impl ItemAssets {
    /// 优先 2x（600x900 原图），退回 1x（300x450）。
    fn capsule_filename(&self) -> Option<&str> {
        self.library_capsule_2x
            .as_deref()
            .or(self.library_capsule.as_deref())
            .filter(|name| !name.is_empty())
    }

    fn poster_url(&self) -> Option<String> {
        let format = self
            .asset_url_format
            .as_deref()
            .filter(|format| !format.is_empty())?;
        let filename = self.capsule_filename()?;
        Some(asset_url(format, filename))
    }
}

/// 商店目录里的单条信息（官方愿望单批量补全用）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreCatalogEntry {
    pub name: Option<String>,
    /// 未知类型为 `None`（调用方保留原判断）
    pub media_kind: Option<MediaKind>,
    pub poster_url: Option<String>,
}

/// 从响应里解析 `appid → 目录信息`。
///
/// Steam 对已下架 / 不可见条目会返回 `appid: 0` 的占位对象，直接跳过。
pub fn parse_store_catalog(raw: &serde_json::Value) -> HashMap<String, StoreCatalogEntry> {
    let Ok(response) = serde_json::from_value::<GetItemsResponse>(raw.clone()) else {
        return HashMap::new();
    };

    response
        .response
        .store_items
        .into_iter()
        .filter_map(|item| {
            let appid = item.appid.filter(|appid| *appid > 0)?;
            let name = item
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string);
            Some((
                appid.to_string(),
                StoreCatalogEntry {
                    name,
                    media_kind: item.store_type.and_then(MediaKind::from_store_browse_type),
                    poster_url: item.assets.poster_url(),
                },
            ))
        })
        .collect()
}

/// 从响应里解析 `appid → 竖版海报 URL`；缺 assets / 缺文件名的条目静默跳过。
pub fn parse_library_capsules(raw: &serde_json::Value) -> HashMap<String, String> {
    parse_store_catalog(raw)
        .into_iter()
        .filter_map(|(appid, entry)| entry.poster_url.map(|url| (appid, url)))
        .collect()
}

/// 把批量目录信息合并到条目上（只覆盖来源确实给了的字段）。
///
/// 返回仍未取到名称的条目数：官方愿望单接口只给 appid，目录失败时这些条目会停留在
/// `Steam App XXX` 占位符。
pub fn apply_store_catalog(
    items: &mut [SourceItem],
    catalog: &HashMap<String, StoreCatalogEntry>,
) -> usize {
    let mut missing = 0usize;
    for item in items {
        if let Some(entry) = catalog.get(&item.external_id) {
            if let Some(name) = &entry.name {
                item.title = name.clone();
            }
            if let Some(kind) = entry.media_kind {
                item.media_kind = kind;
            }
            if let Some(poster) = &entry.poster_url {
                item.poster_url = Some(poster.clone());
            }
        }
        if item.title.trim().is_empty() || item.title.starts_with("Steam App ") {
            missing += 1;
        }
    }
    missing
}

/// `asset_url_format` 里的 `${FILENAME}` 换成具体文件，拼上资产 CDN。
fn asset_url(format: &str, filename: &str) -> String {
    format!(
        "{ASSET_CDN_BASE}/{}",
        format.replace("${FILENAME}", filename)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "response": {
            "store_items": [
                {
                    "appid": 4264260,
                    "name": "示例游戏",
                    "type": 0,
                    "assets": {
                        "asset_url_format": "steam/apps/4264260/${FILENAME}?t=1779968151",
                        "library_capsule": "aaa/library_capsule_schinese.jpg",
                        "library_capsule_2x": "bbb/library_capsule_schinese_2x.jpg"
                    }
                },
                { "appid": 999, "name": "无海报", "type": 4,
                  "assets": { "asset_url_format": "steam/apps/999/${FILENAME}" } },
                { "appid": 1000 },
                { "appid": 2000, "name": "工具", "type": 6 },
                { "appid": 0, "name": "", "type": null }
            ]
        }
    }"#;

    /// 2x 优先、`${FILENAME}` 正确替换、缺 assets 的条目跳过。
    #[test]
    fn parses_library_capsule_urls() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let map = parse_library_capsules(&value);

        assert_eq!(map.len(), 1, "缺 assets / 缺文件名的条目应被跳过：{map:?}");
        assert_eq!(
            map.get("4264260").map(String::as_str),
            Some(concat!(
                "https://shared.akamai.steamstatic.com/store_item_assets/",
                "steam/apps/4264260/bbb/library_capsule_schinese_2x.jpg?t=1779968151"
            )),
        );
    }

    /// 目录解析：本地化名称、整数类型映射，`appid: 0` 的占位对象跳过。
    #[test]
    fn parses_store_catalog_names_and_types() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let catalog = parse_store_catalog(&value);

        let game = catalog.get("4264260").expect("应解析出游戏");
        assert_eq!(game.name.as_deref(), Some("示例游戏"));
        assert_eq!(game.media_kind, Some(MediaKind::Game));
        assert!(game.poster_url.is_some());

        let dlc = catalog.get("999").expect("应解析出 DLC");
        assert_eq!(dlc.media_kind, Some(MediaKind::Dlc));
        assert!(dlc.poster_url.is_none(), "没有 asset 文件名时不造海报 URL");

        let software = catalog.get("2000").expect("应解析出软件");
        assert_eq!(software.media_kind, Some(MediaKind::Software));

        assert!(
            !catalog.contains_key("0"),
            "Steam 对不可见条目返回 appid 0，必须跳过"
        );
    }

    /// 补全：占位标题换成真实名称、DLC 类型生效、海报生效；缺名称的条目被计数。
    #[test]
    fn applies_store_catalog_to_items() {
        let mut items = vec![
            SourceItem {
                external_id: "4264260".to_string(),
                title: "Steam App 4264260".to_string(),
                ..Default::default()
            },
            SourceItem {
                external_id: "999".to_string(),
                title: "Steam App 999".to_string(),
                ..Default::default()
            },
        ];
        let catalog = HashMap::from([(
            "4264260".to_string(),
            StoreCatalogEntry {
                name: Some("示例游戏".to_string()),
                media_kind: Some(MediaKind::Game),
                poster_url: Some("https://example.com/p.jpg".to_string()),
            },
        )]);

        let missing = apply_store_catalog(&mut items, &catalog);
        assert_eq!(missing, 1, "999 没有目录信息，仍是占位符");
        assert_eq!(items[0].title, "示例游戏");
        assert_eq!(
            items[0].poster_url.as_deref(),
            Some("https://example.com/p.jpg")
        );
        assert_eq!(items[1].title, "Steam App 999");
    }

    /// 只有 1x 时退回 1x（部分游戏的文件名不是 library_capsule*，原样使用）。
    #[test]
    fn falls_back_to_1x_capsule() {
        let raw = r#"{"response":{"store_items":[{"appid":1,"assets":{
            "asset_url_format":"steam/apps/1/${FILENAME}","library_capsule":"b/portrait.png"}}]}}"#;
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        let map = parse_library_capsules(&value);

        assert!(
            map["1"].ends_with("steam/apps/1/b/portrait.png"),
            "{}",
            map["1"]
        );
    }

    /// `input_json` 必须被 percent-encode；非数字 appid 跳过。
    #[test]
    fn encodes_input_json_in_url() {
        let appids = vec!["4264260".to_string(), "not-a-number".to_string()];
        let url = store_items_url(&appids, "cn", "schinese");

        assert!(url.starts_with(
            "https://api.steampowered.com/IStoreBrowseService/GetItems/v1/?input_json="
        ));
        assert!(
            url.contains("%22country_code%22%3A%22cn%22"),
            "input_json 必须被编码：{url}"
        );
        assert!(url.contains("%22appid%22%3A4264260"), "{url}");
        assert!(!url.contains("not-a-number"), "非法 appid 应被跳过：{url}");
    }
}
