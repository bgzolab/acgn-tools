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

pub mod appdetails;
pub mod client;
pub mod config;
pub mod fixture;
pub mod util;
pub mod wishlist;

pub use appdetails::AppDetailsData;
pub use client::{SteamClient, WishlistFetch};
pub use config::SteamConfig;
pub use fixture::FixtureData;
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

/// 官方愿望单接口（需要 API Key）。
pub fn official_wishlist_url(api_key: &str, steam_id: &str) -> String {
    format!("{API_BASE}/IWishlistService/GetWishlist/v1/?key={api_key}&steamid={steam_id}")
}

/// 商店详情接口（无需 API Key，但有限流）。
pub fn app_details_url(appid: &str, country: &str, language: &str) -> String {
    format!("{STORE_BASE}/api/appdetails?appids={appid}&cc={country}&l={language}")
}

/// vanity URL 转 steamID64（无需 API Key，解析 XML）。
pub fn vanity_resolve_url(vanity: &str) -> String {
    format!("{COMMUNITY_BASE}/id/{vanity}/?xml=1")
}
