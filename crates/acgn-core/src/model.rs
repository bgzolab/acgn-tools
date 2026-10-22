//! 领域模型。
//!
//! 设计要点：
//!
//! * `Game` 只描述「条目本身」（元数据，来自 Steam / Bangumi / 小黑盒……）；
//! * `LibraryEntry` 描述「我和这个条目的关系」（状态、评分、进度、备注）；
//! * 两者用 `game_id` 1:1 关联，未来要支持「同一游戏在多个平台各有一份」时，
//!   可以再插入一层 holdings，而不必动这两个核心模型。

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Error;

/// 条目在个人库里的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// 愿望单（从 Steam 愿望单导入时的默认状态）
    Wishlist,
    /// 待玩（已入手、排队中）
    Backlog,
    /// 在玩
    Playing,
    /// 已通关 / 已看完
    Completed,
    /// 搁置
    OnHold,
    /// 弃坑
    Dropped,
    /// 忽略（不参与统计）
    Ignored,
}

impl Status {
    /// 用于前端渲染筛选器时保持稳定顺序。
    pub const ALL: [Status; 7] = [
        Status::Wishlist,
        Status::Backlog,
        Status::Playing,
        Status::Completed,
        Status::OnHold,
        Status::Dropped,
        Status::Ignored,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Status::Wishlist => "wishlist",
            Status::Backlog => "backlog",
            Status::Playing => "playing",
            Status::Completed => "completed",
            Status::OnHold => "on_hold",
            Status::Dropped => "dropped",
            Status::Ignored => "ignored",
        }
    }

    /// 中文标签，前端直接显示。
    pub const fn label(self) -> &'static str {
        match self {
            Status::Wishlist => "愿望单",
            Status::Backlog => "待玩",
            Status::Playing => "在玩",
            Status::Completed => "已通关",
            Status::OnHold => "搁置",
            Status::Dropped => "弃坑",
            Status::Ignored => "忽略",
        }
    }

    /// 宽松解析，兼容历史数据与第三方导入的各种写法。
    pub fn from_str_lossy(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        match normalized.as_str() {
            "wishlist" | "wish" | "想要的" => Some(Status::Wishlist),
            "backlog" | "owned" | "library" => Some(Status::Backlog),
            "playing" | "play" | "doing" | "in_progress" => Some(Status::Playing),
            "completed" | "complete" | "finished" | "beaten" | "done" => Some(Status::Completed),
            "on_hold" | "onhold" | "hold" | "paused" => Some(Status::OnHold),
            "dropped" | "drop" | "abandoned" => Some(Status::Dropped),
            "ignored" | "ignore" | "hidden" => Some(Status::Ignored),
            _ => None,
        }
    }
}

impl FromStr for Status {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Status::from_str_lossy(s).ok_or_else(|| Error::Invalid(format!("未知状态: {s}")))
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 数据来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Steam,
    Bangumi,
    /// 小黑盒（社区向，后续接入）
    Xiaoheihe,
    /// 手动录入 / 通用 JSON 导入
    #[default]
    Manual,
}

impl Source {
    pub const ALL: [Source; 4] = [
        Source::Steam,
        Source::Bangumi,
        Source::Xiaoheihe,
        Source::Manual,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Source::Steam => "steam",
            Source::Bangumi => "bangumi",
            Source::Xiaoheihe => "xiaoheihe",
            Source::Manual => "manual",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Source::Steam => "Steam",
            Source::Bangumi => "Bangumi",
            Source::Xiaoheihe => "小黑盒",
            Source::Manual => "手动",
        }
    }

    pub fn from_str_lossy(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "steam" | "steam_wishlist" | "steam_library" => Some(Source::Steam),
            "bangumi" | "bgm" => Some(Source::Bangumi),
            "xiaoheihe" | "xhh" | "heybox" => Some(Source::Xiaoheihe),
            "manual" | "local" | "json" => Some(Source::Manual),
            _ => None,
        }
    }
}

impl FromStr for Source {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Source::from_str_lossy(s).ok_or_else(|| Error::Invalid(format!("未知来源: {s}")))
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 媒介类型。游戏库先落地，漫画 / 动画 / 小说未来复用同一枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    #[default]
    Game,
    Dlc,
    Bundle,
    Software,
    Demo,
    Other,
}

impl MediaKind {
    pub const ALL: [MediaKind; 6] = [
        MediaKind::Game,
        MediaKind::Dlc,
        MediaKind::Bundle,
        MediaKind::Software,
        MediaKind::Demo,
        MediaKind::Other,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            MediaKind::Game => "game",
            MediaKind::Dlc => "dlc",
            MediaKind::Bundle => "bundle",
            MediaKind::Software => "software",
            MediaKind::Demo => "demo",
            MediaKind::Other => "other",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            MediaKind::Game => "游戏",
            MediaKind::Dlc => "DLC",
            MediaKind::Bundle => "捆绑包",
            MediaKind::Software => "软件",
            MediaKind::Demo => "试玩",
            MediaKind::Other => "其他",
        }
    }

    /// 兼容 Steam 的 `type` 字段（以及未来的 `app_type`）。
    pub fn from_steam_type(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "game" => MediaKind::Game,
            "dlc" => MediaKind::Dlc,
            "bundle" | "pack" | "sub" => MediaKind::Bundle,
            "software" | "tool" | "application" => MediaKind::Software,
            "demo" => MediaKind::Demo,
            _ => MediaKind::Other,
        }
    }

    /// Steam `IStoreBrowseService/GetItems` 的整型 `type`：
    /// `0` 游戏、`1/2` 包/捆绑、`4` DLC、`6` 软件，音乐/硬件/视频/系列归到 `Other`。
    ///
    /// 未知值返回 `None`：调用方应保留原判断，而不是把一个正常游戏误判成 `Other`
    /// 后被 `include_non_games` 过滤掉。
    pub fn from_store_browse_type(value: i64) -> Option<Self> {
        match value {
            0 => Some(MediaKind::Game),
            1 | 2 => Some(MediaKind::Bundle),
            3 | 5 | 7 | 8 => Some(MediaKind::Other),
            4 => Some(MediaKind::Dlc),
            6 => Some(MediaKind::Software),
            _ => None,
        }
    }

    pub fn from_str_lossy(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "game" => Some(MediaKind::Game),
            "dlc" | "expansion" => Some(MediaKind::Dlc),
            "bundle" | "pack" => Some(MediaKind::Bundle),
            "software" | "tool" => Some(MediaKind::Software),
            "demo" => Some(MediaKind::Demo),
            "other" => Some(MediaKind::Other),
            _ => None,
        }
    }
}

impl fmt::Display for MediaKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 条目元数据（属于「这个世界」，与我的个人状态无关）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Game {
    pub id: i64,
    pub source: Source,
    /// 来源侧的稳定 ID：Steam 是 appid，Bangumi 是 subject id。
    pub external_id: String,
    pub media_kind: MediaKind,
    pub title: String,
    pub title_zh: Option<String>,
    pub short_description: Option<String>,
    /// 主展示图（Steam 的 header.jpg）
    pub cover_url: Option<String>,
    /// 竖版海报（Steam 的 library_600x900.jpg），列表卡片用
    pub poster_url: Option<String>,
    /// 规范化后的发售日期（YYYY-MM-DD，能解析出来时才填）
    pub release_date: Option<String>,
    /// 原始发售日期文案（"25 Feb, 2022" / "即将推出"）
    pub release_raw: Option<String>,
    /// Steam 好评等级 0..=8，8 最好
    pub review_score: Option<i64>,
    /// 好评描述（"Very Positive"）
    pub review_desc: Option<String>,
    pub review_count: Option<i64>,
    pub is_free: Option<bool>,
    /// 价格统一用「分」存整数，避免浮点误差；币种单独存。
    pub price_currency: Option<String>,
    pub price_initial: Option<i64>,
    pub price_final: Option<i64>,
    pub discount_percent: Option<i64>,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub genres: Vec<String>,
    pub platforms: Vec<String>,
    /// 来源侧的原始标签（Steam 愿望单里的 tags）
    pub tags: Vec<String>,
    /// 最近一次抓取元数据的时间；为空表示还没抓过详情
    pub metadata_updated_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Game {
    /// 折扣价（分）。没有价格信息时为 `None`。
    pub fn effective_price(&self) -> Option<i64> {
        self.price_final.or(self.price_initial)
    }

    /// 是否有折扣。
    pub fn has_discount(&self) -> bool {
        matches!(self.discount_percent, Some(p) if p > 0)
    }
}

/// 「我和这个条目的关系」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryEntry {
    pub game_id: i64,
    pub status: Status,
    /// 来源侧的优先级（Steam 愿望单顺序，越小越靠前）
    pub priority: Option<i64>,
    pub favorite: bool,
    /// 0..=10，10 最好
    pub rating: Option<i64>,
    pub progress_percent: Option<i64>,
    /// 展示 / 统计用的有效时长（分钟）。默认跟随来源，用户手填后以本地为准。
    pub playtime_minutes: Option<i64>,
    /// 最近一次来源导入报告的时长（分钟，Steam `playtime_forever`）。
    /// 用来判断 `playtime_minutes` 是否已被用户手改，以及展示「Steam 记录」。
    pub source_playtime_minutes: Option<i64>,
    /// 最近一次检查到的成就进度（`achieved` / `total`）；`total = 0` 表示该游戏没有成就集
    pub achievements_achieved: Option<i64>,
    pub achievements_total: Option<i64>,
    /// 最近一次检查成就的时间（轮询用：最久没查过的优先）
    pub achievements_checked_at: Option<DateTime<Utc>>,
    pub note: Option<String>,
    /// 来源侧「加入时间」（Steam wishlist 的 added）
    pub source_added_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    /// 最近一次与来源同步的时间
    pub last_synced_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 条目 + 个人状态的组合视图，所有列表接口都返回它。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameWithEntry {
    pub game: Game,
    pub entry: LibraryEntry,
    /// 关联的数据源账号（库存 / 愿望单）。没有账号关联（手动条目、旧数据）时为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<GameAccount>,
}

impl GameWithEntry {
    /// 不带账号关联的构造（账号信息由存储层按需回填）。
    pub fn new(game: Game, entry: LibraryEntry) -> Self {
        Self {
            game,
            entry,
            accounts: Vec::new(),
        }
    }
}

/// 数据源账号（Steam / Bangumi / 小黑盒……的登录身份）。
///
/// 注意：这是**数据源侧的身份**，不是本项目的多用户账号体系。
/// `(source, external_id)` 唯一；`external_id` 对 Steam 是 17 位 steamID64。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub source: Source,
    pub external_id: String,
    /// 来源侧的展示名（Steam 的 `personaname`）
    pub display_name: Option<String>,
    /// 头像 URL（Steam 的 `avatarfull`）
    pub avatar_url: Option<String>,
    pub profile_url: Option<String>,
    /// 资料里的区域代码（Steam 的 `loccountrycode`，小写）
    pub country_code: Option<String>,
    /// 手动覆盖的价格区域；为空时跟随资料
    pub country_override: Option<String>,
    /// 实际取价区域：`country_override` 优先，其次 `country_code`
    pub effective_country: Option<String>,
    /// 来源侧原始资料载荷（有多少存多少，方便以后补字段）
    pub metadata: Option<serde_json::Value>,
    pub metadata_updated_at: Option<DateTime<Utc>>,
    /// 最近一次同步（库存 / 愿望单 / 成就）的时间
    pub last_synced_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Account {
    /// 实际取价区域（`country_override` 优先）。
    pub fn country(&self) -> Option<&str> {
        self.country_override
            .as_deref()
            .or(self.country_code.as_deref())
    }

    /// 展示名兜底：没有昵称时用「Steam 765…」。
    pub fn display_label(&self) -> String {
        self.display_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{} {}", self.source.label(), self.external_id))
    }
}

/// 适配器抓到的一份账号资料（存储层负责合并与落库）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AccountProfile {
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub profile_url: Option<String>,
    /// 资料里的区域代码（Steam `loccountrycode`）
    pub country_code: Option<String>,
    /// 来源原始载荷
    pub metadata: Option<serde_json::Value>,
}

/// 账号列表项：账号 + 关联统计。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountSummary {
    #[serde(flatten)]
    pub account: Account,
    /// 该账号库存里的条目数（含 0 分钟）
    pub owned_count: i64,
    /// 该账号愿望单里的条目数
    pub wishlist_count: i64,
}

/// 游戏 ↔ 账号的关联（随 `GameWithEntry` 返回）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameAccount {
    pub account_id: i64,
    pub source: Source,
    /// 账号的 steamID64 / 来源侧 ID
    pub external_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    /// 该账号的实际取价区域（override ?? 资料区域）
    pub country: Option<String>,
    /// 是否仍在最近一次该账号的愿望单里
    pub in_wishlist: bool,
    /// 是否在该账号的库存里（`source_playtime_minutes` 非空即拥有，0 也算）
    pub owned: bool,
    /// 该账号的累计游玩时长（分钟）
    pub source_playtime_minutes: Option<i64>,
    /// 最近两周游玩时长（分钟）
    pub playtime_2weeks_minutes: Option<i64>,
    /// 最后一次游玩的时刻
    pub last_played_at: Option<DateTime<Utc>>,
    /// 按该账号区域抓到的价格（分）
    pub price_currency: Option<String>,
    pub price_initial: Option<i64>,
    pub price_final: Option<i64>,
    pub discount_percent: Option<i64>,
    pub is_free: Option<bool>,
    /// 非空表示「该账号区域已经问过 Steam」：此时没有价格 = 该区域确实没有售价，
    /// 而不是「还没抓过」。
    pub price_updated_at: Option<DateTime<Utc>>,
    pub last_synced_at: Option<DateTime<Utc>>,
}

impl GameAccount {
    /// 该账号区域的价格（分）。折扣价优先。
    pub fn effective_price(&self) -> Option<i64> {
        self.price_final.or(self.price_initial)
    }
}

/// 一条游戏新闻（来源公告 RSS 的规范化结果）。
///
/// 新闻按 `(game, language, guid)` 唯一；同一份公告换了语言会作为另一条记录保存，
/// 展示层按配置语言过滤。`guid` 是来源侧稳定 ID（Steam 公告通常是详情页 URL）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameNews {
    pub id: i64,
    pub game_id: i64,
    pub source: Source,
    /// 来源侧 ID（Steam 是 appid）
    pub external_id: String,
    pub game_title: String,
    pub game_title_zh: Option<String>,
    pub game_cover_url: Option<String>,
    pub game_poster_url: Option<String>,
    pub language: String,
    pub title: String,
    /// 公告正文的纯文本摘要（HTML 已在适配器里剥离）
    pub summary: Option<String>,
    pub url: String,
    pub author: Option<String>,
    pub published_at: DateTime<Utc>,
    /// 首次抓取到这条新闻的时间
    pub fetched_at: DateTime<Utc>,
}

/// 一次导入 / 同步的运行记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportRun {
    pub id: i64,
    pub source: Source,
    /// `steam_wishlist` / `json` / `metadata_enrich` …
    pub mode: String,
    /// `running` / `ok` / `failed`
    pub status: String,
    pub seen: i64,
    pub added: i64,
    pub updated: i64,
    pub unchanged: i64,
    pub skipped: i64,
    pub missing: i64,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub args: Option<serde_json::Value>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
}

/// 数据源侧的偏好设置（Steam 愿望单顺序等），用于还原「原始排位」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourcePreference {
    pub source: Source,
    /// 来源侧的排序位次
    pub rank: Option<i64>,
    /// 来源侧的优先级
    pub priority: Option<i64>,
}
