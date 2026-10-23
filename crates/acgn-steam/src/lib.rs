//! `acgn-steam`：Steam 数据源适配层。
//!
//! 三条取数路径，按可用性自动选择：
//!
//! | 路径 | 需要 | 拿到什么 |
//! |---|---|---|
//! | `public_json` | 个人资料公开 | 名称、封面、评价、发售日、加入时间、愿望单排位 |
//! | `official_api` | Steam Web API Key | 只有 appid 列表（名称需再补全），但不受隐私设置影响以外的问题 |
//! | `fixture` | 本地 JSON | 完全离线，用于开发 / 演示 / 回归测试 |
//!
//! 所有路径最终都产出 [`acgn_core::SourceItem`]，入库逻辑与其它数据源共用一套。

pub mod achievements;
pub mod appdetails;
pub mod assets;
pub mod client;
pub mod config;
pub mod fixture;
pub mod library;
pub mod news;
pub mod profile;
pub mod util;
pub mod wishlist;

pub use achievements::parse_player_achievements;
pub use appdetails::AppDetailsData;
pub use client::{AppDetailsBatch, ImportFetch, SteamClient};
pub use config::SteamConfig;
pub use fixture::FixtureData;
pub use library::{OwnedGame, OwnedGamesPayload};
pub use news::{NewsFetch, parse_news_feed};
pub use profile::{parse_player_summaries, parse_profile_xml};
pub use wishlist::{WishlistItem, WishlistPayload};

/// Steam 商店 CDN（header / 竖版封面都能直接拼出来，不依赖 hash）。
pub const CDN_BASE: &str = "https://cdn.cloudflare.steamstatic.com/steam/apps";
/// 商店站点。
pub const STORE_BASE: &str = "https://store.steampowered.com";
/// 社区站点（vanity URL 解析用）。
pub const COMMUNITY_BASE: &str = "https://steamcommunity.com";
/// 官方 Web API。
pub const API_BASE: &str = "https://api.steampowered.com";

/// header.jpg：商店横版主图（列表 / 详情页顶部用）。
pub fn header_url(appid: &str) -> String {
    format!("{CDN_BASE}/{appid}/header.jpg")
}

/// library_600x900.jpg：竖版海报（卡片网格用）。
pub fn poster_url(appid: &str) -> String {
    format!("{CDN_BASE}/{appid}/library_600x900.jpg")
}

/// 愿望单公开 JSON 接口（无需 API Key）。
pub fn public_wishlist_url(steam_id: &str, page: u32) -> String {
    format!("{STORE_BASE}/wishlist/profiles/{steam_id}/wishlistdata/?p={page}")
}

/// 官方愿望单接口；`api_key` 与 `cursor` 都是可选的。
///
/// 实测（2026-10）该接口**允许匿名调用**，不需要 API Key；`key` 只在配置了才带上。
/// 用 `Url` 构造并让参数走 percent-encoding：`next_cursor` 是 Steam 给的 opaque
/// token，直接字符串拼接遇到 `+` / `&` / `=` 会把 query 截断。
pub fn official_wishlist_url(
    api_key: Option<&str>,
    steam_id: &str,
    cursor: Option<&str>,
) -> String {
    let mut url = reqwest::Url::parse(&format!("{API_BASE}/IWishlistService/GetWishlist/v1/"))
        .expect("静态 API 地址应当合法");

    {
        let mut pairs = url.query_pairs_mut();
        if let Some(key) = api_key.filter(|key| !key.is_empty()) {
            pairs.append_pair("key", key);
        }
        pairs.append_pair("steamid", steam_id);
        if let Some(cursor) = cursor.filter(|cursor| !cursor.is_empty()) {
            pairs.append_pair("cursor", cursor);
        }
    }

    url.to_string()
}

/// 商店详情接口（无需 API Key，但有限流）。
pub fn app_details_url(appid: &str, country: &str, language: &str) -> String {
    format!("{STORE_BASE}/api/appdetails?appids={appid}&cc={country}&l={language}")
}

/// 游戏公告 RSS（社区站，无需 API Key）。
///
/// 每个 appid 一个 feed；`l` 控制请求语言（Steam 的用户公告内容不一定会翻译，
/// 但请求头 / 站点语言会跟随），`cc` 与商店区域保持一致。
pub fn news_feed_url(appid: &str, country: &str, language: &str) -> String {
    format!("{COMMUNITY_BASE}/games/{appid}/rss/?cc={country}&l={language}")
}

/// 官方库存接口（**必须**带 API Key：匿名调用会被 401 拒绝）。
///
/// `include_played_free_games` 让免费游戏（如 CS2 / Dota）也出现在库存里；
/// 名字与图标由 appdetails 之外的本接口顺带返回（`include_appinfo`）。
pub fn owned_games_url(api_key: &str, steam_id: &str, language: &str) -> String {
    let mut url = reqwest::Url::parse(&format!("{API_BASE}/IPlayerService/GetOwnedGames/v1/"))
        .expect("静态 API 地址应当合法");

    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("key", api_key);
        pairs.append_pair("steamid", steam_id);
        pairs.append_pair("include_appinfo", "1");
        pairs.append_pair("include_played_free_games", "1");
        pairs.append_pair("include_free_sub", "1");
        pairs.append_pair("format", "json");
        pairs.append_pair("language", language);
    }

    url.to_string()
}

/// vanity URL 转 steamID64（无需 API Key，解析 XML）。
pub fn vanity_resolve_url(vanity: &str) -> String {
    format!("{COMMUNITY_BASE}/id/{vanity}/?xml=1")
}

/// 单游戏成就进度（需要 API Key；返回的是 `steam_id` 这个用户的达成情况）。
///
/// 响应较大（每个成就一条），调用方只需要汇总数字；
/// 隐私 / 未拥有时 Steam 返回 `success:false`，由 [`parse_player_achievements`] 转成 `None`。
pub fn player_achievements_url(
    api_key: &str,
    steam_id: &str,
    appid: &str,
    language: &str,
) -> String {
    let mut url = reqwest::Url::parse(&format!(
        "{API_BASE}/ISteamUserStats/GetPlayerAchievements/v1/"
    ))
    .expect("静态 API 地址应当合法");

    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("key", api_key);
        pairs.append_pair("steamid", steam_id);
        pairs.append_pair("appid", appid);
        pairs.append_pair("l", language);
    }

    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `next_cursor` 是 Steam 给的 opaque token：必须 percent-encode，
    /// 否则含 `+` / `&` / `=` 时 query 会被截断，第 2 页起拿不到数据。
    #[test]
    fn official_wishlist_url_encodes_cursor() {
        let url = official_wishlist_url(None, "76561198000000000", None);
        // 没有 Key 时不应出现 key 参数：该接口允许匿名调用
        assert_eq!(
            url,
            "https://api.steampowered.com/IWishlistService/GetWishlist/v1/?steamid=76561198000000000"
        );

        let url = official_wishlist_url(Some("KEY"), "76561198000000000", None);
        assert!(url.contains("key=KEY"), "{url}");

        let url = official_wishlist_url(None, "76561198000000000", Some("a+b&c=d"));
        assert!(
            url.contains("cursor=a%2Bb%26c%3Dd"),
            "cursor 必须被编码：{url}"
        );
        assert!(!url.contains("&c="), "未编码的 & 会截断 query：{url}");
    }

    /// 库存接口必须带上 Key 与关键开关；Key 里的特殊字符也要被正确编码。
    #[test]
    fn owned_games_url_carries_key_and_switches() {
        let url = owned_games_url("A+b&c=d", "76561198000000000", "schinese");
        assert!(
            url.starts_with("https://api.steampowered.com/IPlayerService/GetOwnedGames/v1/?"),
            "{url}"
        );
        assert!(url.contains("key=A%2Bb%26c%3Dd"), "Key 必须被编码：{url}");
        assert!(url.contains("steamid=76561198000000000"), "{url}");
        assert!(url.contains("include_appinfo=1"), "{url}");
        assert!(
            url.contains("include_played_free_games=1"),
            "免费游戏也应算库存：{url}"
        );
        assert!(url.contains("language=schinese"), "{url}");
    }

    /// 成就接口：appid 与 Key 都要正确编码，语言参数要带上。
    #[test]
    fn player_achievements_url_encodes_parameters() {
        let url = player_achievements_url("A+b&c=d", "76561198000000000", "1245620", "schinese");
        assert!(
            url.starts_with(
                "https://api.steampowered.com/ISteamUserStats/GetPlayerAchievements/v1/?"
            ),
            "{url}"
        );
        assert!(url.contains("key=A%2Bb%26c%3Dd"), "{url}");
        assert!(url.contains("steamid=76561198000000000"), "{url}");
        assert!(url.contains("appid=1245620"), "{url}");
        assert!(url.contains("l=schinese"), "{url}");
    }

    /// 新闻 RSS：语言与区域都进 query（`l=` 不能写成示例里的 `=schinese` 笔误）。
    #[test]
    fn news_feed_url_carries_country_and_language() {
        let url = news_feed_url("1250650", "hk", "schinese");
        assert_eq!(
            url,
            "https://steamcommunity.com/games/1250650/rss/?cc=hk&l=schinese"
        );
    }
}
