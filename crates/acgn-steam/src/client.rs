//! Steam HTTP 客户端：ID 解析、愿望单抓取、商店详情补全。
//!
//! 取数优先级：
//!
//! 1. 配置了 `ACGN_STEAM_FIXTURE` → 完全离线，读本地 JSON；
//! 2. 公开愿望单接口（无需 Key，信息最全）；
//! 3. 公开接口被拒且配置了 `STEAM_API_KEY` → 官方接口（只有 appid，名称等需再补全）。

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use acgn_core::{AccountProfile, AchievementProgress, Error, Result, SourceItem};
use futures::future::join_all;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE};
use tracing::{debug, warn};

use crate::achievements::parse_player_achievements;
use crate::appdetails::parse_app_details;
use crate::assets::{STORE_ITEMS_CHUNK, parse_library_capsules, store_items_url};
use crate::config::SteamConfig;
use crate::fixture::FixtureData;
use crate::library::{OwnedGamesPayload, owned_game_to_source_item};
use crate::profile::{
    parse_player_summaries, parse_profile_xml, player_summaries_url, profile_xml_url,
};
use crate::util::extract_xml_tag;
use crate::wishlist::{WishlistPayload, wishlist_to_source_item};
use crate::{
    app_details_url, official_wishlist_url, owned_games_url, player_achievements_url,
    public_wishlist_url, vanity_resolve_url,
};

/// 愿望单最多翻多少页（每页约 100 条，足够 6000 个愿望）。
const MAX_WISHLIST_PAGES: u32 = 60;

/// 官方接口最多翻多少页（分页语义若变化，靠告警暴露而不是静默截断）。
const MAX_OFFICIAL_PAGES: u32 = 60;

/// 商店详情接口的兜底国家：国区不售的条目只有换区才能拿到数据。
const FALLBACK_COUNTRY: &str = "us";

/// reqwest 错误 → 领域错误。
///
/// **必须脱敏**：reqwest 的 `Display` 会带上完整请求 URL（含 query），而官方愿望单接口的
/// API Key 就在 query 里（`?key=...`）。一旦原样透传，密钥会经 warnings → `import_runs`、
/// `ApiError` → HTTP 响应、tracing → 日志三处泄漏，且永久留在数据库的同步历史里。
/// `without_url()` 只去掉 URL 字段，`is_timeout()` / `is_decode()` 等分类信息都保留。
pub(crate) trait ReqwestExt<T> {
    fn net(self) -> Result<T>;
}

impl<T> ReqwestExt<T> for std::result::Result<T, reqwest::Error> {
    fn net(self) -> Result<T> {
        self.map_err(|err| {
            let err = err.without_url();
            if err.is_timeout() {
                Error::network(format!("请求超时: {err}"))
            } else if err.is_decode() {
                Error::upstream(format!("响应解析失败: {err}"))
            } else {
                Error::network(err)
            }
        })
    }
}

/// 计算需要换 `us` 重试的 appid：本块在配置国家下没拿到 capsule 的子集。
/// 配置国家已经是 `us` 时返回空（不再重试）。
fn fallback_candidates(
    chunk: &[String],
    primary: &HashMap<String, String>,
    country: &str,
) -> Vec<String> {
    if country == FALLBACK_COUNTRY {
        return Vec::new();
    }
    chunk
        .iter()
        .filter(|appid| !primary.contains_key(*appid))
        .cloned()
        .collect()
}

/// 上游状态码 → 领域错误：5xx 是上游故障（502 upstream），
/// 其余非成功状态按来源拒绝（422）处理——别把 Steam 宕机误导成「隐私设置问题」。
fn http_error_for(endpoint: &str, status: reqwest::StatusCode) -> Error {
    if status.is_server_error() {
        Error::upstream(format!("{endpoint} 返回 HTTP {status}"))
    } else {
        Error::rejected(format!("{endpoint} 返回 HTTP {status}"))
    }
}

/// 用户输入的 Steam ID 形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdInput {
    /// 17 位 steamID64
    Numeric(String),
    /// 个性域名（vanity URL）
    Vanity(String),
}

/// 解析用户输入的 SteamID / 主页链接 / 个性域名。
pub fn parse_id_input(input: &str) -> Result<IdInput> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Error::invalid("SteamID 不能为空"));
    }

    let lowered = trimmed.to_ascii_lowercase();
    if let Some(rest) = lowered.split("steamcommunity.com/profiles/").nth(1) {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.len() == 17 {
            return Ok(IdInput::Numeric(digits));
        }
        return Err(Error::invalid(format!(
            "链接里的 steamID64 不合法: {trimmed}"
        )));
    }
    if let Some(rest) = trimmed.split("steamcommunity.com/id/").nth(1) {
        let vanity = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
        if !vanity.is_empty() {
            return Ok(IdInput::Vanity(vanity.to_string()));
        }
        return Err(Error::invalid(format!("链接里没有个性域名: {trimmed}")));
    }

    if trimmed.len() == 17 && trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Ok(IdInput::Numeric(trimmed.to_string()));
    }

    if trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && (2..=64).contains(&trimmed.len())
    {
        return Ok(IdInput::Vanity(trimmed.to_string()));
    }

    Err(Error::invalid(format!(
        "无法识别 SteamID：{trimmed}（支持 17 位 steamID64、个性域名或个人主页链接）"
    )))
}

/// 批量补全的结果。
///
/// 把「取不到的条目」和「为什么取不到」分开：
/// `warnings` 给用户看，`last_error` 给调用方区分「已下架(成功但无数据)」和
/// 「被限流/拒绝(应该提示稍后重试)」——否则 429 会被误报成「可能已下架」。
#[derive(Debug)]
pub struct AppDetailsBatch {
    pub items: Vec<SourceItem>,
    pub warnings: Vec<String>,
    pub last_error: Option<Error>,
}

/// 一次抓取的结果（愿望单 / 库存共用）。
#[derive(Debug, Clone)]
pub struct ImportFetch {
    /// 解析后的 steamID64
    pub steam_id: String,
    pub items: Vec<SourceItem>,
    pub warnings: Vec<String>,
    /// `public_json` / `official_api` / `fixture`
    pub source_mode: &'static str,
    /// `numeric` / `vanity` / `fixture`
    pub resolved_via: &'static str,
}

/// Steam 客户端。
pub struct SteamClient {
    http: reqwest::Client,
    config: SteamConfig,
    fixture: Option<FixtureData>,
}

impl SteamClient {
    pub fn new(config: SteamConfig) -> Result<Self> {
        let fixture = config.load_fixture()?;
        let http = reqwest::Client::builder()
            .user_agent(config.user_agent.clone())
            .timeout(config.timeout)
            .gzip(true)
            .build()
            .map_err(Error::network)?;

        Ok(Self {
            http,
            config,
            fixture,
        })
    }

    pub fn config(&self) -> &SteamConfig {
        &self.config
    }

    pub fn mode(&self) -> &'static str {
        self.config.mode()
    }

    pub fn fixture(&self) -> Option<&FixtureData> {
        self.fixture.as_ref()
    }

    /// 解析 ID：数字直接用；个性域名走社区 XML（无需 Key）。
    pub async fn resolve_id(&self, input: &str) -> Result<(String, &'static str, Vec<String>)> {
        let mut warnings = Vec::new();
        match parse_id_input(input)? {
            IdInput::Numeric(id) => Ok((id, "numeric", warnings)),
            IdInput::Vanity(vanity) => {
                if let Some(fixture) = &self.fixture {
                    return match &fixture.steam_id {
                        Some(id) => {
                            warnings.push(format!(
                                "fixture 模式下使用内置 steam_id={id} 代替 vanity `{vanity}`"
                            ));
                            Ok((id.clone(), "fixture", warnings))
                        }
                        None => Err(Error::invalid(
                            "fixture 模式下无法解析个性域名：请在 fixture 里写 steam_id，或直接填 17 位 steamID64",
                        )),
                    };
                }

                let url = vanity_resolve_url(&vanity);
                let body = self
                    .http
                    .get(url)
                    .header(ACCEPT, "application/xml, text/xml")
                    .send()
                    .await
                    .net()?
                    .text()
                    .await
                    .net()?;

                let id = extract_xml_tag(&body, "steamID64").ok_or_else(|| {
                    Error::rejected(format!(
                        "无法解析个性域名 `{vanity}`：主页不存在或未公开（也可以直接填写 17 位 steamID64）"
                    ))
                })?;
                Ok((id, "vanity", warnings))
            }
        }
    }

    /// 抓账号资料（昵称 / 头像 / 主页 / 区域 + 原始载荷）。
    ///
    /// 三条路径：
    /// 1. fixture：`profile.json`（缺失时明确报错，调用方当作告警）；
    /// 2. 配了 API Key：官方 `GetPlayerSummaries`（字段最全，含 `loccountrycode`）；
    /// 3. 没有 Key：社区资料 XML（只有昵称 / 头像 / 主页，没有区域）。
    pub async fn fetch_profile(&self, steam_id: &str) -> Result<AccountProfile> {
        if let Some(fixture) = &self.fixture {
            return fixture.account_profile();
        }

        if let Some(api_key) = self.config.api_key.as_deref().filter(|key| !key.is_empty()) {
            let url = player_summaries_url(api_key, steam_id);
            let response = self
                .http
                .get(url)
                .header(ACCEPT, "application/json")
                .send()
                .await
                .net()?;

            let status = response.status();
            if !status.is_success() {
                return Err(http_error_for("GetPlayerSummaries", status));
            }

            let value: serde_json::Value = response.json().await.net()?;
            return parse_player_summaries(&value).ok_or_else(|| {
                Error::rejected(format!(
                    "Steam 未返回账号 {steam_id} 的资料：请确认 SteamID 正确、资料未隐藏"
                ))
            });
        }

        let url = profile_xml_url(steam_id);
        let response = self
            .http
            .get(url)
            .header(ACCEPT, "application/xml, text/xml")
            .send()
            .await
            .net()?;
        let status = response.status();
        let body = response.text().await.net()?;

        // 未登录 / 被限制时社区会返回 HTML 或登录页，必须报「无法读取」而不是造一个空资料
        if !status.is_success() || !body.contains("<profile") {
            return Err(Error::rejected(format!(
                "无法读取社区资料（HTTP {status}）：配置 STEAM_API_KEY 可稳定获取昵称 / 头像 / 区域"
            )));
        }

        Ok(parse_profile_xml(&body))
    }

    /// 抓取愿望单：公开接口优先，失败且配了 Key 就回退官方接口。
    pub async fn fetch_wishlist(&self, input: &str) -> Result<ImportFetch> {
        let (steam_id, resolved_via, mut warnings) = self.resolve_id(input).await?;

        if let Some(fixture) = &self.fixture {
            let items: Vec<SourceItem> = fixture
                .wishlist_entries()
                .into_iter()
                .map(|(appid, item)| wishlist_to_source_item(&appid, &item))
                .collect();
            warnings.push(format!(
                "离线 fixture 模式：{} 个条目来自 {}",
                items.len(),
                fixture.wishlist_path.display()
            ));
            return Ok(ImportFetch {
                steam_id,
                items,
                warnings,
                source_mode: "fixture",
                resolved_via: "fixture",
            });
        }

        match self.fetch_public_wishlist(&steam_id).await {
            Ok((items, mut extra)) => {
                warnings.append(&mut extra);
                Ok(ImportFetch {
                    steam_id,
                    items,
                    warnings,
                    source_mode: "public_json",
                    resolved_via,
                })
            }
            Err(err) => {
                // 公开接口（带名字/封面）Steam 已对匿名访问收紧；官方接口允许匿名调用，
                // 所以无论有没有配置 Key 都要回退，否则用户会卡在一个无解的错误上。
                warnings.push(format!(
                    "公开愿望单接口不可用（{err}），已回退到官方 Web API{}",
                    if self.config.has_api_key() {
                        "（带 Key）"
                    } else {
                        "（匿名）"
                    }
                ));
                let (items, mut extra) = self.fetch_official_wishlist(&steam_id).await?;
                warnings.append(&mut extra);
                warnings.push(
                    "官方接口只返回 appid：标题先显示为占位符，请用「补全元数据」拉取名称/价格/简介"
                        .to_string(),
                );
                Ok(ImportFetch {
                    steam_id,
                    items,
                    warnings,
                    source_mode: "official_api",
                    resolved_via,
                })
            }
        }
    }

    /// 抓取库存（已拥有游戏 + 游玩时长）。
    ///
    /// 与愿望单不同，官方 `GetOwnedGames` 接口 **不接受匿名调用**（实测 401），
    /// 因此除 fixture 外必须配置 `STEAM_API_KEY`；同时要求「游戏详情」对所有人可见，
    /// 否则 Steam 返回空对象。两者都要给出可操作的错误提示。
    pub async fn fetch_library(&self, input: &str) -> Result<ImportFetch> {
        let (steam_id, resolved_via, mut warnings) = self.resolve_id(input).await?;

        if let Some(fixture) = &self.fixture {
            let Some(path) = fixture.library_path.as_ref().filter(|path| path.exists()) else {
                return Err(Error::rejected(
                    "离线 fixture 未提供 library.json：无法演示库存导入（重新生成 fixture 或改用真实网络）",
                ));
            };
            let games = fixture.library_games();
            let items: Vec<SourceItem> = games.iter().map(owned_game_to_source_item).collect();
            warnings.push(format!(
                "离线 fixture 模式：{} 个条目来自 {}",
                items.len(),
                path.display()
            ));
            return Ok(ImportFetch {
                steam_id,
                items,
                warnings,
                source_mode: "fixture",
                resolved_via: "fixture",
            });
        }

        let Some(api_key) = self.config.api_key.as_deref().filter(|key| !key.is_empty()) else {
            return Err(Error::invalid(
                "导入 Steam 库存需要 API Key（官方 GetOwnedGames 接口不允许匿名调用）：\
                 请设置 STEAM_API_KEY 后重启服务，并确认「游戏详情」隐私为公开",
            ));
        };

        let url = owned_games_url(api_key, &steam_id, &self.config.language);
        let response = self
            .http
            .get(url)
            .header(ACCEPT, "application/json")
            .send()
            .await
            .net()?;

        let status = response.status();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(Error::rejected(
                "Steam 拒绝了库存请求：API Key 无效或已过期（到 https://steamcommunity.com/dev/apikey 重新申请）",
            ));
        }
        if status.as_u16() == 429 {
            return Err(Error::rejected(
                "GetOwnedGames 触发限流（HTTP 429），请稍后再同步库存",
            ));
        }
        if !status.is_success() {
            return Err(http_error_for("GetOwnedGames", status));
        }

        let payload: OwnedGamesPayload = response.json().await.net()?;
        if payload.privacy_blocked() {
            return Err(Error::rejected(
                "Steam 未返回库存：请确认 SteamID 正确，并把「游戏详情」隐私设置为公开",
            ));
        }

        let games = payload.into_games();
        if games.is_empty() {
            warnings.push(
                "GetOwnedGames 返回 0 条：确认「游戏详情」为公开，或该账号确实没有游戏".to_string(),
            );
        }
        let items: Vec<SourceItem> = games.iter().map(owned_game_to_source_item).collect();
        Ok(ImportFetch {
            steam_id,
            items,
            warnings,
            source_mode: "official_api",
            resolved_via,
        })
    }

    /// 抓单个游戏的成就进度（`ISteamUserStats/GetPlayerAchievements`）。
    ///
    /// 返回三态：
    /// * `Ok(Some(progress))`：接口明确回答（`total = 0` = 该游戏没有成就集）；
    /// * `Ok(None)`：取不到（未拥有 / 隐私），调用方只刷新检查时间；
    /// * `Err(SourceRejected)`：限流或 Key 失效（422），调用方应停止本轮；
    ///   `Err` 的其它形态（未知载荷等）由连续失败熔断兜底。
    pub async fn fetch_player_achievements(
        &self,
        steam_id: &str,
        appid: &str,
    ) -> Result<Option<AchievementProgress>> {
        if let Some(fixture) = &self.fixture {
            if fixture
                .achievements_path
                .as_ref()
                .is_some_and(|path| path.exists())
            {
                return Ok(fixture.achievement(appid));
            }
            return Err(Error::rejected(
                "离线 fixture 未提供 achievements.json：无法演示成就同步（重新生成 fixture 或改用真实网络）",
            ));
        }

        let Some(api_key) = self.config.api_key.as_deref().filter(|key| !key.is_empty()) else {
            return Err(Error::invalid(
                "同步成就需要 API Key（官方 GetPlayerAchievements 接口不接受匿名调用）：\
                 请设置 STEAM_API_KEY 后重启服务",
            ));
        };

        let url = player_achievements_url(api_key, steam_id, appid, &self.config.language);
        let response = self
            .http
            .get(url)
            .header(ACCEPT, "application/json")
            .send()
            .await
            .net()?;

        let status = response.status();
        if status.as_u16() == 429 {
            return Err(Error::rejected(
                "GetPlayerAchievements 触发限流（HTTP 429），请稍后再试",
            ));
        }

        // 400（该游戏没有统计）/ 403（未拥有、隐私）都会带可解析的 JSON 答案，
        // 所以先把 body 当载荷解析；只有「不是 JSON」时才按状态码报错。
        let body = response.text().await.net()?;
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
            return parse_player_achievements(&value);
        }

        if matches!(status.as_u16(), 401 | 403) {
            return Err(Error::rejected(
                "Steam 拒绝了成就请求：API Key 无效或已过期（到 https://steamcommunity.com/dev/apikey 重新申请）",
            ));
        }
        if !status.is_success() {
            return Err(http_error_for(
                "GetPlayerAchievements（响应不是 JSON）",
                status,
            ));
        }
        Err(Error::upstream(format!(
            "成就响应解析失败（前 80 字符: {}）",
            body.chars().take(80).collect::<String>()
        )))
    }

    /// 公开接口：分页拉取 `wishlistdata`，按 appid 去重。
    async fn fetch_public_wishlist(
        &self,
        steam_id: &str,
    ) -> Result<(Vec<SourceItem>, Vec<String>)> {
        let mut items = Vec::new();
        let mut warnings = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();

        for page in 0..MAX_WISHLIST_PAGES {
            let url = public_wishlist_url(steam_id, page);
            let response = self
                .http
                .get(url)
                .header(ACCEPT, "application/json")
                .header(ACCEPT_LANGUAGE, self.config.language.as_str())
                .send()
                .await
                .net()?;

            let status = response.status();

            // reqwest 默认跟随重定向：Steam 收紧匿名访问后会把 wishlistdata 302 到商店首页，
            // 拿到的是商店 HTML。这种情况必须报「接口不可用」，不能当成「愿望单为空」，
            // 否则用户会去反复检查自己的隐私设置。
            let final_path = response.url().path().to_string();
            if !final_path.contains("/wishlist/") {
                return Err(Error::rejected(format!(
                    "公开愿望单接口已不再对匿名访问开放（被重定向到 {final_path}）"
                )));
            }
            if status.as_u16() == 403 || status.as_u16() == 401 {
                return Err(Error::rejected(
                    "Steam 拒绝了愿望单请求：请把「个人资料」「游戏详情」「愿望单」都设为公开",
                ));
            }
            if status.as_u16() == 404 {
                if page > 0 {
                    break;
                }
                return Err(Error::rejected(
                    "Steam 返回 404：请确认 SteamID 是否正确，以及个人资料 / 愿望单是否公开",
                ));
            }
            if !status.is_success() {
                return Err(http_error_for("愿望单接口", status));
            }

            let body = response.text().await.net()?;
            let trimmed = body.trim();

            if trimmed.is_empty() || trimmed.starts_with('<') {
                if page == 0 {
                    return Err(Error::rejected(
                        "愿望单为空或不可见：请确认「愿望单」隐私设置为公开（也可以稍后重试）",
                    ));
                }
                break;
            }

            let payload: WishlistPayload = serde_json::from_str(trimmed).map_err(|err| {
                Error::upstream(format!(
                    "愿望单 JSON 解析失败: {err}（前 80 字符: {}）",
                    trimmed.chars().take(80).collect::<String>()
                ))
            })?;

            if payload.is_empty() {
                break;
            }

            let mut fresh = 0usize;
            for (appid, item) in payload.into_entries() {
                match appid {
                    Some(appid) if seen.insert(appid.clone()) => {
                        items.push(wishlist_to_source_item(&appid, &item));
                        fresh += 1;
                    }
                    Some(_) => {}
                    None => warnings.push("有 1 条愿望单数据缺少 appid，已跳过".to_string()),
                }
            }

            debug!(page, fresh, total = items.len(), "愿望单分页");
            if fresh == 0 {
                break;
            }
        }

        if items.is_empty() {
            warnings.push("愿望单接口返回 0 条：确认愿望单不是空的".to_string());
        }
        Ok((items, warnings))
    }

    /// 官方接口：只返回 appid / priority / date_added，带 `next_cursor` 分页。
    ///
    /// 返回 `(items, warnings)`：分页没取全时必须给用户一条告警，不能静默截断。
    async fn fetch_official_wishlist(
        &self,
        steam_id: &str,
    ) -> Result<(Vec<SourceItem>, Vec<String>)> {
        // 该接口实测允许匿名调用（2026-10）；配置了 Key 才带上
        let key = self.config.api_key.as_deref();

        let mut warnings = Vec::new();
        let mut items: Vec<SourceItem> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut cursor: Option<String> = None;

        for page in 0..MAX_OFFICIAL_PAGES {
            let url = official_wishlist_url(key, steam_id, cursor.as_deref());
            let value: serde_json::Value = self
                .http
                .get(url)
                .header(ACCEPT, "application/json")
                .send()
                .await
                .net()?
                .json()
                .await
                .net()?;

            let response = value.get("response");
            let entries = response
                .and_then(|response| response.get("items"))
                .and_then(|items| items.as_array())
                .cloned()
                .unwrap_or_default();

            let mut fresh = 0usize;
            for entry in &entries {
                let Some(appid) = entry
                    .get("appid")
                    .and_then(|v| v.as_i64())
                    .map(|v| v.to_string())
                else {
                    continue;
                };
                if !seen.insert(appid.clone()) {
                    continue;
                }
                items.push(SourceItem {
                    source: acgn_core::Source::Steam,
                    external_id: appid.clone(),
                    title: format!("Steam App {appid}"),
                    cover_url: Some(crate::header_url(&appid)),
                    poster_url: Some(crate::poster_url(&appid)),
                    priority: entry.get("priority").and_then(|v| v.as_i64()),
                    source_added_at: entry
                        .get("date_added")
                        .and_then(|v| v.as_i64())
                        .and_then(|seconds| crate::util::from_unix(Some(seconds))),
                    metadata: Some(entry.clone()),
                    ..Default::default()
                });
                fresh += 1;
            }

            let next = response
                .and_then(|response| response.get("next_cursor"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);

            let Some(next) = next else { break };

            if fresh == 0 {
                // 有游标但没拿到新数据：接口语义可能变了，别再无限翻页
                warnings.push(format!(
                    "官方愿望单接口返回了 next_cursor 但没有新条目（cursor={next}）：分页语义可能已变化，本次导入可能不完整"
                ));
                break;
            }
            if page + 1 >= MAX_OFFICIAL_PAGES {
                warnings.push(format!(
                    "官方愿望单接口翻页超过 {MAX_OFFICIAL_PAGES} 页仍未结束，本次导入可能不完整"
                ));
                break;
            }
            cursor = Some(next);
        }

        Ok((items, warnings))
    }

    /// 单条 appdetails（限流安全：调用方控制节奏）。
    ///
    /// 国区不售的条目在 `cc=cn` 下会返回 `success:false`（联机商店不卖，但数据存在），
    /// 因此配置的国家拿不到时用 `us` 再试一次——否则用户会看到一堆「可能已下架」的假失败。
    async fn fetch_app_details_live(&self, appid: &str) -> Result<Option<SourceItem>> {
        let primary = self
            .fetch_app_details_for_country(appid, &self.config.country)
            .await?;
        if primary.is_some() || self.config.country == FALLBACK_COUNTRY {
            return Ok(primary);
        }

        self.fetch_app_details_for_country(appid, FALLBACK_COUNTRY)
            .await
    }

    async fn fetch_app_details_for_country(
        &self,
        appid: &str,
        country: &str,
    ) -> Result<Option<SourceItem>> {
        let url = app_details_url(appid, country, &self.config.language);
        let response = self
            .http
            .get(url)
            .header(ACCEPT, "application/json")
            .header(ACCEPT_LANGUAGE, self.config.language.as_str())
            .send()
            .await
            .net()?;

        let status = response.status();
        if status.as_u16() == 429 {
            return Err(Error::rejected(
                "appdetails 触发限流（HTTP 429），请稍后再补全",
            ));
        }
        if !status.is_success() {
            return Err(http_error_for("appdetails", status));
        }

        let value: serde_json::Value = response.json().await.net()?;
        let entry = value.get(appid).cloned().unwrap_or(serde_json::Value::Null);
        Ok(parse_app_details(&entry).map(|data| data.to_source_item(appid)))
    }

    /// 用新资产接口补竖版海报：`library_capsule`（600x900）覆盖 `poster_url`。
    ///
    /// 旧式 `.../library_600x900.jpg` 对成人 / 区域受限内容已经 404，而 `appdetails`
    /// 只有横版 header。海报属于锦上添花：失败只记日志，不影响元数据主流程。
    ///
    /// 拿不到新地址时把 `poster_url` 置 `None`，而不是留着旧式模板：合并对 `None`
    /// 保留本地值（`take_authoritative`），已存过的好海报不会被一次接口故障覆盖；
    /// 首次补全的条目该次没有海报，交给前端封面兜底。
    async fn fill_poster_assets(&self, items: &mut [SourceItem]) {
        if items.is_empty() {
            return;
        }
        let appids: Vec<String> = items.iter().map(|item| item.external_id.clone()).collect();
        let capsules = self.store_library_capsules(&appids).await;
        for item in items.iter_mut() {
            item.poster_url = capsules.get(&item.external_id).cloned();
        }
    }

    /// 取一批 appid 的竖版海报 URL（尽力而为：单个分块失败只记日志，返回已取到的部分）。
    ///
    /// 先配置国家；整块拿不到的再切 `us` 重试——国区上下文会把成人内容的 assets
    /// 整个隐藏，而不是逐条报错。任意一次请求失败都不能作废已取到的结果。
    async fn store_library_capsules(&self, appids: &[String]) -> HashMap<String, String> {
        let mut capsules = HashMap::new();
        let chunk_count = appids.len().div_ceil(STORE_ITEMS_CHUNK);
        for (index, chunk) in appids.chunks(STORE_ITEMS_CHUNK).enumerate() {
            match self
                .store_items_for_country(chunk, &self.config.country)
                .await
            {
                Ok(primary) => {
                    let missing = fallback_candidates(chunk, &primary, &self.config.country);
                    capsules.extend(primary);

                    if !missing.is_empty() {
                        // 回退失败只损失缺的这几条：主国家结果照常保留
                        match self
                            .store_items_for_country(&missing, FALLBACK_COUNTRY)
                            .await
                        {
                            Ok(fallback) => capsules.extend(fallback),
                            Err(err) => {
                                warn!(error = %err, "竖版海报换区重试失败（保留主国家结果）")
                            }
                        }
                    }
                }
                Err(err) => warn!(error = %err, "竖版海报请求失败（跳过该批）"),
            }

            // 与 appdetails 一样按节流间隔排队（只在还有下一块时等待），
            // 避免批量补全时连续打请求触发限流
            if index + 1 < chunk_count && self.config.enrich_delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.config.enrich_delay_ms)).await;
            }
        }
        capsules
    }

    async fn store_items_for_country(
        &self,
        appids: &[String],
        country: &str,
    ) -> Result<HashMap<String, String>> {
        let url = store_items_url(appids, country, &self.config.language);
        let response = self
            .http
            .get(url)
            .header(ACCEPT, "application/json")
            .header(ACCEPT_LANGUAGE, self.config.language.as_str())
            .send()
            .await
            .net()?;

        let status = response.status();
        if !status.is_success() {
            return Err(Error::upstream(format!(
                "IStoreBrowseService 返回 HTTP {status}"
            )));
        }

        let value: serde_json::Value = response.json().await.net()?;
        Ok(parse_library_capsules(&value))
    }

    /// 批量补全：fixture 直接读文件，线上按并发 + 间隔节流。
    ///
    /// 使用配置区域（国区不售时回退 `us`），并补竖版海报。
    pub async fn app_details_batch(&self, appids: &[String]) -> Result<AppDetailsBatch> {
        self.app_details_batch_inner(appids, None, true).await
    }

    /// 以指定区域抓一批详情（账号区域价格用）。
    ///
    /// 刻意**不做** `us` 回退、也不抓海报：账号区域不售时应该记录「该区域没有售价」，
    /// 拿美元价冒充账号区域价会误导用户。
    pub async fn app_details_batch_for_country(
        &self,
        appids: &[String],
        country: &str,
    ) -> Result<AppDetailsBatch> {
        self.app_details_batch_inner(appids, Some(country), false)
            .await
    }

    async fn app_details_batch_inner(
        &self,
        appids: &[String],
        country: Option<&str>,
        fill_posters: bool,
    ) -> Result<AppDetailsBatch> {
        if let Some(fixture) = &self.fixture {
            let mut items = Vec::new();
            let mut warnings = Vec::new();
            for appid in appids {
                match fixture.app_details(appid) {
                    Some(response) if response.success => match &response.data {
                        Some(data) => items.push(data.to_source_item(appid)),
                        None => warnings.push(format!("{appid}: fixture 里 data 为空")),
                    },
                    _ => warnings.push(format!(
                        "{appid}: fixture ({}) 里没有详情数据",
                        fixture
                            .appdetails_path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "-".to_string())
                    )),
                }
            }
            return Ok(AppDetailsBatch {
                items,
                warnings,
                last_error: None,
            });
        }

        let mut items = Vec::new();
        let mut warnings = Vec::new();
        let mut last_error: Option<Error> = None;
        let concurrency = self.config.enrich_concurrency.max(1);

        'outer: for chunk in appids.chunks(concurrency) {
            let futures = chunk.iter().map(|appid| {
                let appid = appid.clone();
                let client = self;
                async move {
                    let result = match country {
                        Some(country) => {
                            client.fetch_app_details_for_country(&appid, country).await
                        }
                        None => client.fetch_app_details_live(&appid).await,
                    };
                    (appid, result)
                }
            });
            let results = join_all(futures).await;

            for (appid, result) in results {
                match result {
                    Ok(Some(item)) => items.push(item),
                    Ok(None) => warnings.push(format!("{appid}: Steam 未返回详情（可能已下架）")),
                    Err(err) => {
                        let rejected = matches!(err, Error::SourceRejected(_));
                        warnings.push(format!("{appid}: {err}"));
                        if rejected {
                            warnings.push(
                                "已触发 Steam 限流，剩余条目稍后重试即可（已完成的会保留）"
                                    .to_string(),
                            );
                        }
                        last_error = Some(err);
                        if rejected {
                            break 'outer;
                        }
                    }
                }
            }

            if self.config.enrich_delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.config.enrich_delay_ms)).await;
            }
        }

        // 竖版海报在新资产接口里（appdetails 只有横版 header）；
        // 旧式 library_600x900 路径对成人 / 区域受限内容已经 404
        if fill_posters {
            self.fill_poster_assets(&mut items).await;
        }

        Ok(AppDetailsBatch {
            items,
            warnings,
            last_error,
        })
    }

    /// 抓一个 app 的详情（详情页「刷新元数据」用）。
    ///
    /// 语义区分：Steam 明确返回「没有这个 app」（已下架）→ `Ok(None)`；
    /// 限流/拒绝 → `Err(SourceRejected)`，让上层返回 422「稍后重试」而不是「可能已下架」。
    pub async fn app_details_one(&self, appid: &str) -> Result<Option<SourceItem>> {
        let batch = self.app_details_batch(&[appid.to_string()]).await?;
        if batch.items.is_empty() {
            if let Some(err) = batch.last_error {
                return Err(err);
            }
            warn!(appid, warnings = ?batch.warnings, "未取到 appdetails");
            return Ok(None);
        }
        Ok(batch.items.into_iter().next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P1 回归：reqwest 的错误 Display 带完整请求 URL（含 query），而官方接口的
    /// API Key 就在 query 里——`net()` 必须脱敏，否则密钥会进日志/import_runs/API 响应。
    #[tokio::test]
    async fn net_redacts_url_and_query() {
        let client = reqwest::Client::new();
        let err = client
            .get("http://127.0.0.1:1/IWishlistService/GetWishlist/v1/?key=SECRET_KEY_FOR_TEST&steamid=1")
            .send()
            .await
            .net()
            .expect_err("连接失败应当报错");

        let text = err.to_string();
        assert!(
            !text.contains("SECRET_KEY_FOR_TEST"),
            "错误信息里不能出现 API Key：{text}"
        );
        assert!(
            !text.contains("IWishlistService"),
            "错误信息里不应包含请求 URL：{text}"
        );
        assert!(
            matches!(err, acgn_core::Error::Network(_)),
            "应归类为网络错误，实际：{err:?}"
        );
    }

    #[test]
    fn parses_id_inputs() {
        assert_eq!(
            parse_id_input("76561197960287930").unwrap(),
            IdInput::Numeric("76561197960287930".into())
        );
        assert_eq!(
            parse_id_input("https://steamcommunity.com/profiles/76561197960287930/").unwrap(),
            IdInput::Numeric("76561197960287930".into())
        );
        assert_eq!(
            parse_id_input("https://steamcommunity.com/id/gaben/").unwrap(),
            IdInput::Vanity("gaben".into())
        );
        assert_eq!(
            parse_id_input("gaben").unwrap(),
            IdInput::Vanity("gaben".into())
        );
        assert!(parse_id_input("").is_err());
        assert!(parse_id_input("这是一个非法输入 with spaces").is_err());
    }

    fn appid_list(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    /// 只有「本块没拿到」的 appid 才进入 `us` 重试；已是 us 时不重试。
    #[test]
    fn fallback_candidates_only_returns_missing_appids() {
        let chunk = appid_list(&["1", "2", "3"]);
        let mut primary = HashMap::new();
        primary.insert("1".to_string(), "capsule-1".to_string());
        primary.insert("3".to_string(), "capsule-3".to_string());

        assert_eq!(
            fallback_candidates(&chunk, &primary, "cn"),
            vec!["2".to_string()],
            "命中过的 appid 不应再发起 us 重试"
        );
        assert!(
            fallback_candidates(&chunk, &primary, FALLBACK_COUNTRY).is_empty(),
            "配置国家已是 us 时不再重试"
        );
    }

    /// 全命中无需换区；整块缺失则整块回退。
    #[test]
    fn fallback_candidates_handles_full_hits_and_total_misses() {
        let chunk = appid_list(&["1", "2"]);
        let mut all = HashMap::new();
        all.insert("1".to_string(), "x".to_string());
        all.insert("2".to_string(), "y".to_string());

        assert!(fallback_candidates(&chunk, &all, "cn").is_empty());
        assert_eq!(fallback_candidates(&chunk, &HashMap::new(), "cn"), chunk);
    }

    /// fixture 缺 `library.json` / `achievements.json` 时必须明确报错，
    /// 不能静默当成「0 条库存」/「所有游戏都没有成就」（P2 回归）。
    #[tokio::test]
    async fn fixture_missing_files_fail_loudly() {
        let dir = std::env::temp_dir().join(format!("acgn-client-fixture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        std::fs::write(
            dir.join("wishlist.json"),
            r#"{"steam_id":"76561198000000000","wishlist":{}}"#,
        )
        .expect("写 wishlist fixture");

        let client = SteamClient::new(SteamConfig {
            fixture: Some(dir.clone()),
            ..Default::default()
        })
        .expect("客户端应能初始化");

        let err = client
            .fetch_library("76561198000000000")
            .await
            .expect_err("缺 library.json 必须报错");
        assert!(err.to_string().contains("library.json"), "{err}");

        let err = client
            .fetch_player_achievements("76561198000000000", "1")
            .await
            .expect_err("缺 achievements.json 必须报错");
        assert!(err.to_string().contains("achievements.json"), "{err}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 5xx 是上游故障（502 upstream），4xx 才是来源拒绝（422）：
    /// 别把 Steam 宕机提示成「可能是隐私设置问题」。
    #[test]
    fn server_errors_are_upstream_not_rejected() {
        assert!(matches!(
            http_error_for("X", reqwest::StatusCode::BAD_GATEWAY),
            Error::Upstream(_)
        ));
        assert!(matches!(
            http_error_for("X", reqwest::StatusCode::SERVICE_UNAVAILABLE),
            Error::Upstream(_)
        ));
        assert!(matches!(
            http_error_for("X", reqwest::StatusCode::FORBIDDEN),
            Error::SourceRejected(_)
        ));
    }
}
