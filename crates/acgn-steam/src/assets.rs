//! 商店资产（`IStoreBrowseService`）：竖版海报等。
//!
//! `appdetails` 只给横版 `header.jpg`；竖版 `library_capsule`（600x900）要从
//! `api.steampowered.com/IStoreBrowseService/GetItems/v1` 取（匿名可用、支持批量）。
//!
//! 为什么不用旧式路径 `cdn.cloudflare.steamstatic.com/steam/apps/{id}/library_600x900.jpg`：
//! 实测它对一部分游戏直接 404（成人 / 区域受限内容为主），卡片会退化成文字占位。
//!
//! 注意：在国区上下文（`cc=cn`）下，这类游戏的 assets 会被**整体隐藏**（不是逐条 404），
//! 所以调用方拿不到时要换 `us` 再试——与 `appdetails` 的国家回退是同一套思路。

use std::collections::HashMap;

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
}

/// 从响应里解析 `appid → 竖版海报 URL`；缺 assets / 缺文件名的条目静默跳过。
pub fn parse_library_capsules(raw: &serde_json::Value) -> HashMap<String, String> {
    let Ok(response) = serde_json::from_value::<GetItemsResponse>(raw.clone()) else {
        return HashMap::new();
    };

    response
        .response
        .store_items
        .into_iter()
        .filter_map(|item| {
            let appid = item.appid?;
            let format = item
                .assets
                .asset_url_format
                .as_deref()
                .filter(|format| !format.is_empty())?;
            let filename = item.assets.capsule_filename()?;
            Some((appid.to_string(), asset_url(format, filename)))
        })
        .collect()
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
                    "assets": {
                        "asset_url_format": "steam/apps/4264260/${FILENAME}?t=1779968151",
                        "library_capsule": "aaa/library_capsule_schinese.jpg",
                        "library_capsule_2x": "bbb/library_capsule_schinese_2x.jpg"
                    }
                },
                { "appid": 999, "assets": { "asset_url_format": "steam/apps/999/${FILENAME}" } },
                { "appid": 1000 }
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
