//! 游戏列表 / 详情 / 手工编辑。

use acgn_core::{
    EntryPatch, Error, GameListQuery, GameWithEntry, MediaKind, Page, Result, Source, Status,
};
use chrono::Utc;
use sqlx::{QueryBuilder, Sqlite};

use crate::Db;
use crate::error::ResultExt;
use crate::rows::{COUNT_GAME_ENTRY, GameEntryRow, SELECT_GAME_ENTRY};

/// 列表筛选条件（与 HTTP query / Tauri command 解耦）。
#[derive(Debug, Clone, Default)]
pub struct GameFilters {
    pub status: Option<Status>,
    pub source: Option<Source>,
    pub media_kind: Option<MediaKind>,
    pub q: Option<String>,
    pub favorite: Option<bool>,
}

impl GameFilters {
    pub fn from_query(query: &GameListQuery) -> Self {
        Self {
            status: query.status,
            source: query.source,
            media_kind: query.media_kind,
            q: query.q.clone(),
            favorite: query.favorite,
        }
    }
}

/// 排序白名单：只允许这几个字段进入 SQL，避免注入。
fn order_sql(sort: Option<&str>) -> &'static str {
    match sort.unwrap_or("added_desc") {
        "added_asc" => " ORDER BY e.source_added_at IS NULL, e.source_added_at ASC, g.id ASC",
        "added_desc" => " ORDER BY e.source_added_at IS NULL, e.source_added_at DESC, g.id DESC",
        "updated_desc" => " ORDER BY e.updated_at DESC, g.id DESC",
        "title" => " ORDER BY g.title COLLATE NOCASE ASC",
        "review_desc" => {
            " ORDER BY g.review_score IS NULL, g.review_score DESC, g.review_count DESC"
        }
        "rating_desc" => " ORDER BY e.rating IS NULL, e.rating DESC, e.updated_at DESC",
        "priority" => " ORDER BY e.priority IS NULL, e.priority ASC, e.source_added_at ASC",
        "playtime_desc" => {
            " ORDER BY e.playtime_minutes IS NULL, e.playtime_minutes DESC, g.id DESC"
        }
        "price_desc" => {
            " ORDER BY COALESCE(g.price_final, g.price_initial) IS NULL, COALESCE(g.price_final, g.price_initial) DESC"
        }
        _ => " ORDER BY e.source_added_at IS NULL, e.source_added_at DESC, g.id DESC",
    }
}

fn apply_filters(qb: &mut QueryBuilder<'_, Sqlite>, filters: &GameFilters) {
    qb.push(" WHERE 1 = 1");

    if let Some(status) = filters.status {
        qb.push(" AND e.status = ").push_bind(status.as_str());
    }
    if let Some(source) = filters.source {
        qb.push(" AND g.source = ").push_bind(source.as_str());
    }
    if let Some(kind) = filters.media_kind {
        qb.push(" AND g.media_kind = ").push_bind(kind.as_str());
    }
    if filters.favorite == Some(true) {
        qb.push(" AND e.favorite = 1");
    }
    if let Some(term) = filters
        .q
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let like = format!("%{term}%");
        qb.push(" AND (g.title LIKE ")
            .push_bind(like.clone())
            .push(" OR COALESCE(g.title_zh, '') LIKE ")
            .push_bind(like.clone())
            .push(" OR COALESCE(g.developers, '') LIKE ")
            .push_bind(like.clone())
            .push(" OR COALESCE(g.publishers, '') LIKE ")
            .push_bind(like)
            .push(" OR COALESCE(g.tags, '') LIKE ")
            .push_bind(format!("%{term}%"))
            .push(")");
    }
}

impl Db {
    /// 分页列表。
    pub async fn list_games(
        &self,
        filters: &GameFilters,
        sort: Option<&str>,
        page: i64,
        per_page: i64,
    ) -> Result<Page<GameWithEntry>> {
        let page = page.max(1);
        let per_page = per_page.clamp(1, 500);

        let mut qb = QueryBuilder::new(SELECT_GAME_ENTRY);
        apply_filters(&mut qb, filters);
        qb.push(order_sql(sort));
        qb.push(" LIMIT ").push_bind(per_page);
        qb.push(" OFFSET ").push_bind((page - 1) * per_page);

        let rows: Vec<GameEntryRow> = qb.build_query_as().fetch_all(&self.pool).await.db()?;

        let mut count_qb = QueryBuilder::new(COUNT_GAME_ENTRY);
        apply_filters(&mut count_qb, filters);
        let total: i64 = count_qb
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .db()?;

        let items = rows
            .into_iter()
            .map(GameEntryRow::into_domain)
            .collect::<Result<Vec<_>>>()?;

        Ok(Page::new(items, total, page, per_page))
    }

    /// 单条详情。
    pub async fn get_game(&self, id: i64) -> Result<Option<GameWithEntry>> {
        let mut qb = QueryBuilder::new(SELECT_GAME_ENTRY);
        qb.push(" WHERE g.id = ").push_bind(id);

        let row: Option<GameEntryRow> =
            qb.build_query_as().fetch_optional(&self.pool).await.db()?;

        row.map(GameEntryRow::into_domain).transpose()
    }

    /// 按来源 + 外部 ID 查详情（导入去重用）。
    pub async fn get_game_by_external(
        &self,
        source: Source,
        external_id: &str,
    ) -> Result<Option<GameWithEntry>> {
        let mut qb = QueryBuilder::new(SELECT_GAME_ENTRY);
        qb.push(" WHERE g.source = ")
            .push_bind(source.as_str())
            .push(" AND g.external_id = ")
            .push_bind(external_id);

        let row: Option<GameEntryRow> =
            qb.build_query_as().fetch_optional(&self.pool).await.db()?;

        row.map(GameEntryRow::into_domain).transpose()
    }

    /// 手工编辑个人状态。
    ///
    /// 语义约定：
    ///
    /// * 字段缺省 = 不改动；
    /// * `rating` / `progress_percent` / `playtime_minutes` 传负数 = 清空；
    /// * `note` 传空串 = 清空；
    /// * 状态切到在玩 / 已通关时，自动补 `started_at` / `finished_at`；
    /// * **手工修改过的状态不会再被导入覆盖**（导入只更新元数据）。
    pub async fn update_entry(&self, id: i64, patch: &EntryPatch) -> Result<GameWithEntry> {
        let existing = self
            .get_game(id)
            .await?
            .ok_or_else(|| Error::not_found(format!("game #{id}")))?;

        let now = Utc::now();
        let mut entry = existing.entry.clone();
        let previous_status = entry.status;

        if let Some(status) = patch.status {
            entry.status = status;
        }
        if let Some(favorite) = patch.favorite {
            entry.favorite = favorite;
        }
        if let Some(rating) = patch.rating {
            entry.rating = if rating < 0 {
                None
            } else {
                Some(rating.min(10))
            };
        }
        if let Some(progress) = patch.progress_percent {
            entry.progress_percent = if progress < 0 {
                None
            } else {
                Some(progress.min(100))
            };
        }
        if let Some(playtime) = patch.playtime_minutes {
            entry.playtime_minutes = if playtime < 0 { None } else { Some(playtime) };
        }
        if let Some(note) = patch.note.as_deref() {
            entry.note = if note.trim().is_empty() {
                None
            } else {
                Some(note.to_string())
            };
        }
        if let Some(priority) = patch.priority {
            entry.priority = if priority < 0 { None } else { Some(priority) };
        }

        if entry.status != previous_status {
            match entry.status {
                Status::Playing => {
                    entry.started_at.get_or_insert(now);
                    entry.finished_at = None;
                }
                Status::Completed => {
                    entry.started_at.get_or_insert(now);
                    entry.finished_at = Some(now);
                    if entry.progress_percent.is_none() {
                        entry.progress_percent = Some(100);
                    }
                }
                _ => {}
            }
        }

        entry.updated_at = now;

        sqlx::query(
            "UPDATE library_entries SET
                status = ?, priority = ?, favorite = ?, rating = ?, progress_percent = ?,
                playtime_minutes = ?, note = ?, started_at = ?, finished_at = ?, updated_at = ?
             WHERE game_id = ?",
        )
        .bind(entry.status.as_str())
        .bind(entry.priority)
        .bind(entry.favorite)
        .bind(entry.rating)
        .bind(entry.progress_percent)
        .bind(entry.playtime_minutes)
        .bind(entry.note.clone())
        .bind(entry.started_at)
        .bind(entry.finished_at)
        .bind(entry.updated_at)
        .bind(id)
        .execute(&self.pool)
        .await
        .db()?;

        Ok(GameWithEntry {
            game: existing.game,
            entry,
        })
    }

    /// 全库导出（备份 / 未来同步用）。
    pub async fn export_all(&self) -> Result<Vec<GameWithEntry>> {
        let page = self
            .list_games(&GameFilters::default(), Some("added_desc"), 1, 100_000)
            .await?;
        Ok(page.items)
    }
}
