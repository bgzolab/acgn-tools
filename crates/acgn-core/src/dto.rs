//! 跨层传输对象：数据源 → 存储层 → HTTP 层 → 前端。
//!
//! 约定：JSON 一律 `snake_case`（Rust 侧的字段名原样输出），前端 TS 类型与之对齐。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{Game, GameWithEntry, ImportRun, LibraryEntry, MediaKind, Source, Status};

/// 数据源产出的统一条目。
///
/// 这是「接入新社区」的唯一契约：只要能 produce `SourceItem`，就能入库、去重、合并。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceItem {
    #[serde(default)]
    pub source: Source,
    /// 来源侧稳定 ID（Steam appid / Bangumi subject id / 小黑盒 game id）
    pub external_id: String,
    #[serde(default)]
    pub media_kind: MediaKind,
    /// 主标题。为空表示「来源没提供主标题」：
    /// 合并时保留本地已有标题，新建时退回 `title_zh`，都没有才用占位符。
    pub title: String,
    /// 中文名（Steam 的 `l=schinese`、Bangumi 的 `name_cn`）
    pub title_zh: Option<String>,
    pub short_description: Option<String>,
    pub cover_url: Option<String>,
    pub poster_url: Option<String>,
    pub release_date: Option<String>,
    pub release_raw: Option<String>,
    pub review_score: Option<i64>,
    pub review_desc: Option<String>,
    pub review_count: Option<i64>,
    pub is_free: Option<bool>,
    pub price_currency: Option<String>,
    /// 单位：分
    pub price_initial: Option<i64>,
    pub price_final: Option<i64>,
    pub discount_percent: Option<i64>,
    #[serde(default)]
    pub developers: Vec<String>,
    #[serde(default)]
    pub publishers: Vec<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub platforms: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// 新建条目时的初始状态；`None` 表示沿用默认（愿望单）。
    pub status: Option<Status>,
    /// 来源侧优先级 / 排位
    pub priority: Option<i64>,
    /// 来源侧「加入时间」
    pub source_added_at: Option<DateTime<Utc>>,
    /// 来源侧记录的累计游玩时长（分钟，Steam 的 `playtime_forever`）。
    ///
    /// 这是「来源测量值」而不是个人状态：存储层把它记进
    /// `library_entries.source_playtime_minutes`；是否采纳为展示值由合并规则决定
    /// （用户手填过的时长优先，见 `docs/design.md` §4.5）。
    pub playtime_minutes: Option<i64>,
    /// 来源侧原始载荷，落库到 `games.metadata_json`，方便以后补字段
    pub metadata: Option<serde_json::Value>,
    /// 这一份数据的详细程度（决定能否覆盖已有的封面等字段）
    #[serde(default)]
    pub metadata_level: MetadataLevel,
}

impl SourceItem {
    /// 用于日志的简短标识。
    pub fn key(&self) -> String {
        format!("{}:{}", self.source.as_str(), self.external_id)
    }
}

/// 元数据档位。
///
/// 列表接口（Steam 愿望单）只能给出通用封面 URL，详情接口（appdetails）能给出
/// 带 hash 的正式封面。合并时「低档不能覆盖高档已有的值」，否则会出现
/// 「导入愿望单 → 补全详情 → 再导入愿望单」把封面来回改写、每次同步都误报更新的问题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataLevel {
    /// 列表级：名称、评价、发售日、通用封面
    #[default]
    List,
    /// 完整级：详情页拿到的全部字段（简介、开发商、价格、正式封面……）
    Full,
}

impl MetadataLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            MetadataLevel::List => "list",
            MetadataLevel::Full => "full",
        }
    }

    pub const fn is_full(self) -> bool {
        matches!(self, MetadataLevel::Full)
    }
}

/// 导入 / 同步的可选行为。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportOptions {
    /// 是否收下 DLC（默认收）
    #[serde(default = "default_true")]
    pub include_dlc: bool,
    /// 是否收下捆绑包 / 软件 / 试玩（默认不收，多半是噪声）
    #[serde(default)]
    pub include_non_games: bool,
    /// 只统计不落库（用于预览）
    #[serde(default)]
    pub dry_run: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            include_dlc: true,
            include_non_games: false,
            dry_run: false,
        }
    }
}

/// 备份还原用的条目：即 `GET /api/export/json` 里 `items` 的元素格式
///（`{"game": {…}, "entry": {…}}`）。
///
/// 与面向「接入新数据源」的扁平 [`SourceItem`] 不同，它描述的是一条**完整记录** ——
/// 元数据加个人状态 —— 用于把导出文件原样灌回来。两者走不同的存储入口：
/// `SourceItem` 走合并导入，`RestoreItem` 走还原（文件优先，不参与来源合并）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestoreItem {
    pub game: Game,
    /// 缺失时按「愿望单、无个人状态」处理
    #[serde(default)]
    pub entry: Option<LibraryEntry>,
}

impl From<GameWithEntry> for RestoreItem {
    fn from(value: GameWithEntry) -> Self {
        Self {
            game: value.game,
            entry: Some(value.entry),
        }
    }
}

/// 一次导入的结果汇总。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportSummary {
    pub run_id: i64,
    pub source: Source,
    pub mode: String,
    /// 数据源实际返回的条目数
    pub seen: i64,
    pub added: i64,
    pub updated: i64,
    pub unchanged: i64,
    pub skipped: i64,
    /// 上次在愿望单、这次不在了的条目数（可能已购买 / 被移除）
    pub missing: i64,
    pub warnings: Vec<String>,
    pub duration_ms: i64,
    /// 本次涉及的部分 game id，前端用来高亮「刚导入的」
    pub sample_ids: Vec<i64>,
    /// 数据实际来源：`live` / `fixture` / `official_api` / `json`
    pub source_mode: String,
}

/// 分页包装。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, total: i64, page: i64, per_page: i64) -> Self {
        let per_page = per_page.max(1);
        let total_pages = (total + per_page - 1) / per_page;
        Self {
            items,
            total,
            page,
            per_page,
            total_pages,
        }
    }
}

/// 列表查询参数（HTTP query → 存储层）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GameListQuery {
    pub status: Option<Status>,
    pub source: Option<Source>,
    pub media_kind: Option<MediaKind>,
    /// 关键词：匹配标题 / 中文名 / 开发商 / 发行商
    pub q: Option<String>,
    /// `added_desc`（默认） / `added_asc` / `updated_desc` / `title` / `review_desc` / `rating_desc` / `priority`
    pub sort: Option<String>,
    pub favorite: Option<bool>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

impl GameListQuery {
    pub fn page(&self) -> i64 {
        self.page.unwrap_or(1).max(1)
    }

    pub fn per_page(&self) -> i64 {
        self.per_page.unwrap_or(48).clamp(1, 200)
    }

    pub fn offset(&self) -> i64 {
        (self.page() - 1) * self.per_page()
    }
}

/// 手工编辑个人状态时的补丁。
///
/// 约定：字段缺失或 `null` = 不改动；`note` 传空串 = 清空备注。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EntryPatch {
    pub status: Option<Status>,
    pub favorite: Option<bool>,
    pub rating: Option<i64>,
    pub progress_percent: Option<i64>,
    pub playtime_minutes: Option<i64>,
    pub note: Option<String>,
    pub priority: Option<i64>,
}

/// 批量删除请求（`POST /api/games/delete`）。
///
/// 删除是**本地动作**：来源里仍然存在的条目，下次导入会当作新条目重新加回来。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteGamesRequest {
    pub ids: Vec<i64>,
}

/// 批量删除结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteGamesResponse {
    /// 实际删除的行数（未知 id 静默忽略）
    pub deleted: i64,
}

/// 统计信息（看板页）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub total_games: i64,
    pub by_status: Vec<StatusCount>,
    pub by_source: Vec<SourceCount>,
    pub favorite_count: i64,
    pub total_playtime_minutes: i64,
    pub average_rating: Option<f64>,
    /// 愿望单参考价格（按币种分组）
    pub wishlist_value: Vec<MoneyBucket>,
    pub recent_imports: Vec<ImportRun>,
    pub recently_added: Vec<GameWithEntry>,
    pub playing: Vec<GameWithEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusCount {
    pub status: Status,
    pub label: String,
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceCount {
    pub source: Source,
    pub label: String,
    pub count: i64,
}

/// 按币种聚合的金额（单位：分）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoneyBucket {
    pub currency: String,
    pub list_price_cents: i64,
    pub current_price_cents: i64,
    pub item_count: i64,
}

/// 单个游戏的成就进度（Steam `GetPlayerAchievements` 的汇总）。
///
/// `total = 0` 表示这个游戏没有成就集（不是「取不到」——取不到用 `None` 表达）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AchievementProgress {
    pub achieved: i64,
    pub total: i64,
}

impl AchievementProgress {
    /// 是否全成就（`total > 0` 且全部达成）。
    pub fn is_complete(&self) -> bool {
        self.total > 0 && self.achieved >= self.total
    }
}

/// 一次成就同步的结果汇总。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AchievementSyncSummary {
    /// 本次拿到成就数据的条目数（该账号确实拥有且有统计）
    pub checked: i64,
    /// 因「全成就」被标记为已通关的条数
    pub promoted: i64,
    /// 拿到数据但没有变化（进度不满 / 没有成就集）
    pub unchanged: i64,
    /// 该账号没有这个游戏（未拥有 / 隐私）：取不到进度，只刷新了检查时间
    pub unavailable: i64,
    /// 请求失败（网络 / 接口异常），下次会重试
    pub failed: i64,
    /// 本轮结束后仍未检查过的条目数（再点一次继续）
    pub pending: i64,
    pub source_mode: String,
    pub duration_ms: i64,
    pub warnings: Vec<String>,
}

/// 元数据补全的结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnrichSummary {
    pub requested: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub skipped: i64,
    pub warnings: Vec<String>,
    pub source_mode: String,
    pub duration_ms: i64,
}
