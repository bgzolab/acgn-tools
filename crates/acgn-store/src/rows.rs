//! 数据库行（db-shaped）与领域模型（domain-shaped）之间的映射。
//!
//! 行结构体里所有枚举都以 `String` 存放，转换时才解析成领域枚举，
//! 这样领域层不需要依赖 sqlx，解析失败也能给出可读错误。

use acgn_core::{
    Account, AccountSummary, Error, Game, GameAccount, GameWithEntry, ImportRun, LibraryEntry,
    MediaKind, Result, Source, Status,
};
use chrono::{DateTime, Utc};

/// games ⋈ library_entries 的联合行，列名即字段名（`g_` / `e_` 前缀区分同名列）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GameEntryRow {
    pub id: i64,
    pub source: String,
    pub external_id: String,
    pub media_kind: String,
    pub title: String,
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
    pub price_initial: Option<i64>,
    pub price_final: Option<i64>,
    pub discount_percent: Option<i64>,
    pub developers: String,
    pub publishers: String,
    pub genres: String,
    pub platforms: String,
    pub tags: String,
    /// 来源侧原始载荷（JSON 文本），用于以后补字段 / 排查
    pub metadata_json: Option<String>,
    pub metadata_updated_at: Option<DateTime<Utc>>,
    pub g_created_at: DateTime<Utc>,
    pub g_updated_at: DateTime<Utc>,

    pub status: String,
    pub priority: Option<i64>,
    pub favorite: bool,
    pub rating: Option<i64>,
    pub progress_percent: Option<i64>,
    pub playtime_minutes: Option<i64>,
    pub source_playtime_minutes: Option<i64>,
    pub achievements_achieved: Option<i64>,
    pub achievements_total: Option<i64>,
    pub achievements_checked_at: Option<DateTime<Utc>>,
    pub note: Option<String>,
    pub source_added_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub e_created_at: DateTime<Utc>,
    pub e_updated_at: DateTime<Utc>,
}

impl GameEntryRow {
    pub fn into_domain(self) -> Result<GameWithEntry> {
        let source = Source::from_str_lossy(&self.source)
            .ok_or_else(|| Error::storage(format!("非法 source: {}", self.source)))?;
        let media_kind = MediaKind::from_str_lossy(&self.media_kind).unwrap_or(MediaKind::Other);
        let status = Status::from_str_lossy(&self.status)
            .ok_or_else(|| Error::storage(format!("非法 status: {}", self.status)))?;

        let game = Game {
            id: self.id,
            source,
            external_id: self.external_id,
            media_kind,
            title: self.title,
            title_zh: self.title_zh,
            short_description: self.short_description,
            cover_url: self.cover_url,
            poster_url: self.poster_url,
            release_date: self.release_date,
            release_raw: self.release_raw,
            review_score: self.review_score,
            review_desc: self.review_desc,
            review_count: self.review_count,
            is_free: self.is_free,
            price_currency: self.price_currency,
            price_initial: self.price_initial,
            price_final: self.price_final,
            discount_percent: self.discount_percent,
            developers: json_strings(&self.developers),
            publishers: json_strings(&self.publishers),
            genres: json_strings(&self.genres),
            platforms: json_strings(&self.platforms),
            tags: json_strings(&self.tags),
            metadata_updated_at: self.metadata_updated_at,
            created_at: self.g_created_at,
            updated_at: self.g_updated_at,
        };

        let entry = LibraryEntry {
            game_id: self.id,
            status,
            priority: self.priority,
            favorite: self.favorite,
            rating: self.rating,
            progress_percent: self.progress_percent,
            playtime_minutes: self.playtime_minutes,
            source_playtime_minutes: self.source_playtime_minutes,
            achievements_achieved: self.achievements_achieved,
            achievements_total: self.achievements_total,
            achievements_checked_at: self.achievements_checked_at,
            note: self.note,
            source_added_at: self.source_added_at,
            started_at: self.started_at,
            finished_at: self.finished_at,
            last_synced_at: self.last_synced_at,
            created_at: self.e_created_at,
            updated_at: self.e_updated_at,
        };

        Ok(GameWithEntry::new(game, entry))
    }
}

/// accounts 行。
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct AccountRow {
    pub id: i64,
    pub source: String,
    pub external_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub profile_url: Option<String>,
    pub country_code: Option<String>,
    pub country_override: Option<String>,
    pub metadata_json: Option<String>,
    pub metadata_updated_at: Option<DateTime<Utc>>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AccountRow {
    pub fn into_domain(self) -> Result<Account> {
        let source = Source::from_str_lossy(&self.source)
            .ok_or_else(|| Error::storage(format!("非法 source: {}", self.source)))?;
        let effective_country = self
            .country_override
            .clone()
            .or_else(|| self.country_code.clone());
        let metadata = self
            .metadata_json
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok());

        Ok(Account {
            id: self.id,
            source,
            external_id: self.external_id,
            display_name: self.display_name,
            avatar_url: self.avatar_url,
            profile_url: self.profile_url,
            country_code: self.country_code,
            country_override: self.country_override,
            effective_country,
            metadata,
            metadata_updated_at: self.metadata_updated_at,
            last_synced_at: self.last_synced_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

/// accounts + 关联计数（账号列表用）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccountSummaryRow {
    pub id: i64,
    pub source: String,
    pub external_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub profile_url: Option<String>,
    pub country_code: Option<String>,
    pub country_override: Option<String>,
    pub metadata_json: Option<String>,
    pub metadata_updated_at: Option<DateTime<Utc>>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub owned_count: i64,
    pub wishlist_count: i64,
}

impl AccountSummaryRow {
    pub fn into_domain(self) -> Result<AccountSummary> {
        let owned_count = self.owned_count;
        let wishlist_count = self.wishlist_count;
        let account = AccountRow {
            id: self.id,
            source: self.source,
            external_id: self.external_id,
            display_name: self.display_name,
            avatar_url: self.avatar_url,
            profile_url: self.profile_url,
            country_code: self.country_code,
            country_override: self.country_override,
            metadata_json: self.metadata_json,
            metadata_updated_at: self.metadata_updated_at,
            last_synced_at: self.last_synced_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
        .into_domain()?;

        Ok(AccountSummary {
            account,
            owned_count,
            wishlist_count,
        })
    }
}

/// 游戏 ↔ 账号关联行（带账号展示字段，随条目返回）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GameAccountRow {
    pub game_id: i64,
    pub account_id: i64,
    pub source: String,
    pub external_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub country: Option<String>,
    pub in_wishlist: bool,
    pub source_playtime_minutes: Option<i64>,
    pub playtime_2weeks_minutes: Option<i64>,
    pub last_played_at: Option<DateTime<Utc>>,
    pub price_currency: Option<String>,
    pub price_initial: Option<i64>,
    pub price_final: Option<i64>,
    pub discount_percent: Option<i64>,
    pub is_free: Option<bool>,
    pub price_updated_at: Option<DateTime<Utc>>,
    pub last_synced_at: Option<DateTime<Utc>>,
}

impl GameAccountRow {
    pub fn into_domain(self) -> Result<GameAccount> {
        let source = Source::from_str_lossy(&self.source)
            .ok_or_else(|| Error::storage(format!("非法 source: {}", self.source)))?;
        let owned = self.source_playtime_minutes.is_some();

        Ok(GameAccount {
            account_id: self.account_id,
            source,
            external_id: self.external_id,
            display_name: self.display_name,
            avatar_url: self.avatar_url,
            country: self.country,
            in_wishlist: self.in_wishlist,
            owned,
            source_playtime_minutes: self.source_playtime_minutes,
            playtime_2weeks_minutes: self.playtime_2weeks_minutes,
            last_played_at: self.last_played_at,
            price_currency: self.price_currency,
            price_initial: self.price_initial,
            price_final: self.price_final,
            discount_percent: self.discount_percent,
            is_free: self.is_free,
            price_updated_at: self.price_updated_at,
            last_synced_at: self.last_synced_at,
        })
    }
}

/// `game_accounts` 的关联数据（导入合并时用，不含展示字段）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccountLinkRow {
    pub in_wishlist: bool,
    pub source_playtime_minutes: Option<i64>,
    pub playtime_2weeks_minutes: Option<i64>,
    pub last_played_at: Option<DateTime<Utc>>,
}

/// `game_accounts ⋈ accounts` 的固定列清单（与 [`GameAccountRow`] 对齐）。
pub(crate) const SELECT_GAME_ACCOUNT: &str = r#"
SELECT
    ga.game_id                AS game_id,
    ga.account_id             AS account_id,
    a.source                  AS source,
    a.external_id             AS external_id,
    a.display_name            AS display_name,
    a.avatar_url              AS avatar_url,
    LOWER(COALESCE(a.country_override, a.country_code)) AS country,
    ga.in_wishlist            AS in_wishlist,
    ga.source_playtime_minutes AS source_playtime_minutes,
    ga.playtime_2weeks_minutes AS playtime_2weeks_minutes,
    ga.last_played_at         AS last_played_at,
    ga.price_currency         AS price_currency,
    ga.price_initial          AS price_initial,
    ga.price_final            AS price_final,
    ga.discount_percent       AS discount_percent,
    ga.is_free                AS is_free,
    ga.price_updated_at       AS price_updated_at,
    ga.last_synced_at         AS last_synced_at
FROM game_accounts ga
JOIN accounts a ON a.id = ga.account_id
"#;

/// import_runs 行。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ImportRunRow {
    pub id: i64,
    pub source: String,
    pub mode: String,
    pub status: String,
    pub seen: i64,
    pub added: i64,
    pub updated: i64,
    pub unchanged: i64,
    pub skipped: i64,
    pub missing: i64,
    pub warnings: String,
    pub error: Option<String>,
    pub args: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
}

impl ImportRunRow {
    pub fn into_domain(self) -> Result<ImportRun> {
        let source = Source::from_str_lossy(&self.source)
            .ok_or_else(|| Error::storage(format!("非法 source: {}", self.source)))?;

        Ok(ImportRun {
            id: self.id,
            source,
            mode: self.mode,
            status: self.status,
            seen: self.seen,
            added: self.added,
            updated: self.updated,
            unchanged: self.unchanged,
            skipped: self.skipped,
            missing: self.missing,
            warnings: json_strings(&self.warnings),
            error: self.error,
            args: self
                .args
                .as_deref()
                .and_then(|raw| serde_json::from_str(raw).ok()),
            started_at: self.started_at,
            finished_at: self.finished_at,
            duration_ms: self.duration_ms,
        })
    }
}

/// 把存成 JSON 文本的字符串数组读回来；坏了就当空数组（不阻塞列表接口）。
pub(crate) fn json_strings(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// 把字符串数组写成 JSON 文本。
pub(crate) fn strings_json(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
}

/// `games ⋈ library_entries` 的固定列清单（含别名，保持与 [`GameEntryRow`] 对齐）。
pub(crate) const SELECT_GAME_ENTRY: &str = r#"
SELECT
    g.id                  AS id,
    g.source              AS source,
    g.external_id         AS external_id,
    g.media_kind          AS media_kind,
    g.title               AS title,
    g.title_zh            AS title_zh,
    g.short_description   AS short_description,
    g.cover_url           AS cover_url,
    g.poster_url          AS poster_url,
    g.release_date        AS release_date,
    g.release_raw         AS release_raw,
    g.review_score        AS review_score,
    g.review_desc         AS review_desc,
    g.review_count        AS review_count,
    g.is_free             AS is_free,
    g.price_currency      AS price_currency,
    g.price_initial       AS price_initial,
    g.price_final         AS price_final,
    g.discount_percent    AS discount_percent,
    g.developers          AS developers,
    g.publishers          AS publishers,
    g.genres              AS genres,
    g.platforms           AS platforms,
    g.tags                AS tags,
    g.metadata_json       AS metadata_json,
    g.metadata_updated_at AS metadata_updated_at,
    g.created_at          AS g_created_at,
    g.updated_at          AS g_updated_at,
    e.status              AS status,
    e.priority            AS priority,
    e.favorite            AS favorite,
    e.rating              AS rating,
    e.progress_percent    AS progress_percent,
    e.playtime_minutes    AS playtime_minutes,
    e.source_playtime_minutes AS source_playtime_minutes,
    e.achievements_achieved AS achievements_achieved,
    e.achievements_total  AS achievements_total,
    e.achievements_checked_at AS achievements_checked_at,
    e.note                AS note,
    e.source_added_at     AS source_added_at,
    e.started_at          AS started_at,
    e.finished_at         AS finished_at,
    e.last_synced_at      AS last_synced_at,
    e.created_at          AS e_created_at,
    e.updated_at          AS e_updated_at
FROM games g
JOIN library_entries e ON e.game_id = g.id
"#;

pub(crate) const COUNT_GAME_ENTRY: &str =
    "SELECT COUNT(*) FROM games g JOIN library_entries e ON e.game_id = g.id";
