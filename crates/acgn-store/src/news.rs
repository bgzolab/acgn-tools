//! 游戏新闻：Steam 公告 RSS 的归档与查询。
//!
//! 与导入不同，新闻不是「来源里有 / 没有」的合并关系，而是**归档**：
//!
//! * 每个已拥有游戏对应一个 Steam 公告 feed；抓到的条目按 guid 幂等写入；
//! * feed 只返回最近若干条，旧公告继续留在本地，不会被删；
//! * 同步状态按 `(game, language)` 记录，失败只影响「新鲜度」，不影响已入库内容；
//! * 列表只读本地库，网络请求全部发生在显式的同步接口里。
//!
//! 「已拥有」的判定与库存 `missing` 口径一致：账号关联里 `source_playtime_minutes`
//! 非空即为拥有（0 分钟也算）；没有账号关联的旧数据回退到条目级的来源时长。

use std::collections::HashMap;

use acgn_core::{Error, GameNews, NewsEntry, NewsStatus, Page, Result, Source};
use chrono::{DateTime, Utc};
use sqlx::QueryBuilder;

use crate::Db;
use crate::error::ResultExt;

/// 一次新闻写入的计数（不包含「来源里消失」，新闻永远不删除）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NewsUpsert {
    pub added: i64,
    pub updated: i64,
    pub unchanged: i64,
}

/// 已入库的一条新闻（比较用）。
type StoredNews = (
    String,
    Option<String>,
    String,
    Option<String>,
    DateTime<Utc>,
);
/// `store_news` 读取的原始行。
type StoredNewsRow = (
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    DateTime<Utc>,
);

/// 待同步的新闻源：一个游戏 = 一个 feed。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsTarget {
    pub game_id: i64,
    pub external_id: String,
    pub title: String,
}

/// 已拥有 Steam 游戏（新闻同步的目标集合）。
///
/// 新闻只覆盖「我拥有的」游戏，愿望单不抓：
/// * 有账号关联时，以该游戏任一账号的 `source_playtime_minutes IS NOT NULL` 为准；
/// * 没有账号关联（旧数据 / 账号已删除）时，回退到条目级的来源时长。
const OWNED_STEAM_GAMES: &str = r#"
WITH owned AS (
    SELECT g.id, g.external_id, g.title
    FROM games g
    JOIN library_entries e ON e.game_id = g.id
    WHERE g.source = 'steam'
      AND (
        EXISTS (
            SELECT 1 FROM game_accounts ga
            WHERE ga.game_id = g.id AND ga.source_playtime_minutes IS NOT NULL
        )
        OR (
            NOT EXISTS (SELECT 1 FROM game_accounts ga WHERE ga.game_id = g.id)
            AND e.source_playtime_minutes IS NOT NULL
        )
      )
)
"#;

/// `game_news ⋈ games` 的联合行。
#[derive(Debug, Clone, sqlx::FromRow)]
struct GameNewsRow {
    id: i64,
    game_id: i64,
    source: String,
    external_id: String,
    game_title: String,
    game_title_zh: Option<String>,
    game_cover_url: Option<String>,
    game_poster_url: Option<String>,
    language: String,
    title: String,
    summary: Option<String>,
    url: String,
    author: Option<String>,
    published_at: DateTime<Utc>,
    fetched_at: DateTime<Utc>,
}

impl GameNewsRow {
    fn into_domain(self) -> Result<GameNews> {
        let source = Source::from_str_lossy(&self.source)
            .ok_or_else(|| Error::storage(format!("非法 source: {}", self.source)))?;

        Ok(GameNews {
            id: self.id,
            game_id: self.game_id,
            source,
            external_id: self.external_id,
            game_title: self.game_title,
            game_title_zh: self.game_title_zh,
            game_cover_url: self.game_cover_url,
            game_poster_url: self.game_poster_url,
            language: self.language,
            title: self.title,
            summary: self.summary,
            url: self.url,
            author: self.author,
            published_at: self.published_at,
            fetched_at: self.fetched_at,
        })
    }
}

impl Db {
    /// 新闻列表：按发布时间倒序，可选只看某个条目。
    pub async fn list_news(
        &self,
        language: &str,
        game_id: Option<i64>,
        page: i64,
        per_page: i64,
    ) -> Result<Page<GameNews>> {
        let page = page.max(1);
        let per_page = per_page.clamp(1, 100);

        let mut qb = QueryBuilder::new(
            "SELECT n.id AS id, n.game_id AS game_id, g.source AS source,
                    g.external_id AS external_id, g.title AS game_title,
                    g.title_zh AS game_title_zh, g.cover_url AS game_cover_url,
                    g.poster_url AS game_poster_url, n.language AS language,
                    n.title AS title, n.summary AS summary, n.url AS url,
                    n.author AS author, n.published_at AS published_at,
                    n.fetched_at AS fetched_at
             FROM game_news n JOIN games g ON g.id = n.game_id
             WHERE n.language = ",
        );
        qb.push_bind(language);
        if let Some(game_id) = game_id {
            qb.push(" AND n.game_id = ").push_bind(game_id);
        }
        qb.push(" ORDER BY n.published_at DESC, n.id DESC LIMIT ")
            .push_bind(per_page)
            .push(" OFFSET ")
            .push_bind((page - 1) * per_page);

        let rows: Vec<GameNewsRow> = qb.build_query_as().fetch_all(&self.pool).await.db()?;

        let mut count_qb = QueryBuilder::new(
            "SELECT COUNT(*) FROM game_news n JOIN games g ON g.id = n.game_id
             WHERE n.language = ",
        );
        count_qb.push_bind(language);
        if let Some(game_id) = game_id {
            count_qb.push(" AND n.game_id = ").push_bind(game_id);
        }
        let total: i64 = count_qb
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .db()?;

        let items = rows
            .into_iter()
            .map(GameNewsRow::into_domain)
            .collect::<Result<Vec<_>>>()?;

        Ok(Page::new(items, total, page, per_page))
    }

    /// 待同步的新闻源：从没抓过 → 上次失败 → 成功但抓得最久的。
    pub async fn news_targets(&self, language: &str) -> Result<Vec<NewsTarget>> {
        let sql = format!(
            "{OWNED_STEAM_GAMES}
             SELECT o.id, o.external_id, o.title
             FROM owned o
             LEFT JOIN news_feeds f ON f.game_id = o.id AND f.language = ?
             ORDER BY f.last_fetched_at IS NOT NULL,
                      f.last_error IS NULL,
                      f.last_fetched_at ASC,
                      o.id ASC"
        );

        let rows: Vec<(i64, String, String)> = sqlx::query_as(&sql)
            .bind(language)
            .fetch_all(&self.pool)
            .await
            .db()?;

        Ok(rows
            .into_iter()
            .map(|(game_id, external_id, title)| NewsTarget {
                game_id,
                external_id,
                title,
            })
            .collect())
    }

    /// 单个游戏的新闻源；不是已拥有的 Steam 游戏时返回 `None`。
    pub async fn news_target(&self, game_id: i64) -> Result<Option<NewsTarget>> {
        let sql = format!(
            "{OWNED_STEAM_GAMES}
             SELECT o.id, o.external_id, o.title
             FROM owned o
             WHERE o.id = ?
             LIMIT 1"
        );
        let row: Option<(i64, String, String)> = sqlx::query_as(&sql)
            .bind(game_id)
            .fetch_optional(&self.pool)
            .await
            .db()?;

        Ok(row.map(|(game_id, external_id, title)| NewsTarget {
            game_id,
            external_id,
            title,
        }))
    }

    /// 写入一个 feed 的新闻（幂等）。
    ///
    /// * `guid` 已存在且内容一致 → `unchanged`，不写库；
    /// * `guid` 已存在但标题 / 摘要 / 链接 / 作者 / 时间变化 → `updated`；
    /// * 新 `guid` → `added`；
    /// * feed 这次没返回的旧新闻 **不删除**（归档）。
    pub async fn store_news(
        &self,
        game_id: i64,
        language: &str,
        entries: &[NewsEntry],
        now: DateTime<Utc>,
    ) -> Result<NewsUpsert> {
        let existing: Vec<StoredNewsRow> = sqlx::query_as(
            "SELECT guid, title, summary, url, author, published_at
                 FROM game_news WHERE game_id = ? AND language = ?",
        )
        .bind(game_id)
        .bind(language)
        .fetch_all(&self.pool)
        .await
        .db()?;

        let mut known: HashMap<String, StoredNews> = existing
            .into_iter()
            .map(|(guid, title, summary, url, author, published_at)| {
                (guid, (title, summary, url, author, published_at))
            })
            .collect();

        let mut counts = NewsUpsert::default();
        let mut seen: HashMap<&str, ()> = HashMap::new();
        let mut tx = self.pool.begin().await.db()?;

        for entry in entries {
            let guid = entry.guid.trim();
            let title = entry.title.trim();
            let url = entry.url.trim();
            if guid.is_empty() || title.is_empty() || url.is_empty() {
                continue;
            }
            // 同一个 feed 内偶尔会重复出现同一 guid：只写第一次。
            if seen.insert(guid, ()).is_some() {
                continue;
            }

            match known.remove(guid) {
                Some((old_title, old_summary, old_url, old_author, old_published)) => {
                    let changed = old_title != title
                        || old_summary.as_deref() != entry.summary.as_deref()
                        || old_url != url
                        || old_author.as_deref() != entry.author.as_deref()
                        || old_published.timestamp_micros()
                            != entry.published_at.timestamp_micros();
                    if changed {
                        sqlx::query(
                            "UPDATE game_news SET title = ?, summary = ?, url = ?, author = ?,
                                published_at = ?, updated_at = ?
                             WHERE game_id = ? AND language = ? AND guid = ?",
                        )
                        .bind(title)
                        .bind(&entry.summary)
                        .bind(url)
                        .bind(&entry.author)
                        .bind(entry.published_at)
                        .bind(now)
                        .bind(game_id)
                        .bind(language)
                        .bind(guid)
                        .execute(&mut *tx)
                        .await
                        .db()?;
                        counts.updated += 1;
                    } else {
                        counts.unchanged += 1;
                    }
                }
                None => {
                    let result = sqlx::query(
                        "INSERT INTO game_news (
                            game_id, language, guid, title, summary, url, author,
                            published_at, fetched_at, created_at, updated_at
                         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                         ON CONFLICT (game_id, language, guid) DO NOTHING",
                    )
                    .bind(game_id)
                    .bind(language)
                    .bind(guid)
                    .bind(title)
                    .bind(&entry.summary)
                    .bind(url)
                    .bind(&entry.author)
                    .bind(entry.published_at)
                    .bind(now)
                    .bind(now)
                    .bind(now)
                    .execute(&mut *tx)
                    .await
                    .db()?;
                    // 并发同步同一游戏时，另一个请求可能已经插入：按实际结果计数。
                    if result.rows_affected() > 0 {
                        counts.added += 1;
                    } else {
                        counts.unchanged += 1;
                    }
                }
            }
        }

        tx.commit().await.db()?;
        Ok(counts)
    }

    /// 记录一次成功的 feed 抓取。
    pub async fn mark_news_feed_success(
        &self,
        game_id: i64,
        language: &str,
        feed_url: &str,
        item_count: i64,
        now: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO news_feeds (
                game_id, language, feed_url, last_fetched_at, last_success_at,
                last_error, item_count, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, NULL, ?, ?, ?)
             ON CONFLICT (game_id, language) DO UPDATE SET
                feed_url = excluded.feed_url,
                last_fetched_at = excluded.last_fetched_at,
                last_success_at = excluded.last_success_at,
                last_error = NULL,
                item_count = excluded.item_count,
                updated_at = excluded.updated_at",
        )
        .bind(game_id)
        .bind(language)
        .bind(feed_url)
        .bind(now)
        .bind(now)
        .bind(item_count)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    /// 记录一次失败的 feed 抓取：保留上次成功时间与已入库新闻。
    pub async fn mark_news_feed_failure(
        &self,
        game_id: i64,
        language: &str,
        feed_url: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO news_feeds (
                game_id, language, feed_url, last_fetched_at, last_success_at,
                last_error, item_count, created_at, updated_at
             ) VALUES (?, ?, ?, ?, NULL, ?, 0, ?, ?)
             ON CONFLICT (game_id, language) DO UPDATE SET
                feed_url = excluded.feed_url,
                last_fetched_at = excluded.last_fetched_at,
                last_error = excluded.last_error,
                updated_at = excluded.updated_at",
        )
        .bind(game_id)
        .bind(language)
        .bind(feed_url)
        .bind(now)
        .bind(error)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    /// 服务启动时把上次进程遗留的「进行中」新闻同步记录标记为失败。
    ///
    /// 全量同步可能跑几分钟，进程重启/崩溃会留下 `status = 'running'` 的
    /// `steam_news` 运行记录；不清掉的话同步历史里会永远显示「进行中」。
    pub async fn fail_stale_news_runs(&self, error: &str) -> Result<i64> {
        let result = sqlx::query(
            "UPDATE import_runs SET status = 'failed', error = ?, finished_at = ?
             WHERE mode = 'steam_news' AND status = 'running'",
        )
        .bind(error)
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .db()?;
        Ok(result.rows_affected() as i64)
    }

    /// 新闻源覆盖率（当前语言）。
    pub async fn news_status(&self, language: &str) -> Result<NewsStatus> {
        let sql = format!(
            "{OWNED_STEAM_GAMES},
             lang_feeds AS (
                SELECT f.last_success_at, f.last_error
                FROM news_feeds f JOIN owned o ON o.id = f.game_id
                WHERE f.language = ?
             )
             SELECT
                (SELECT COUNT(*) FROM owned) AS owned_games,
                (SELECT COUNT(*) FROM lang_feeds WHERE last_success_at IS NOT NULL) AS synced_games,
                (SELECT COUNT(*) FROM lang_feeds WHERE last_error IS NOT NULL) AS failed_games,
                (SELECT COUNT(*) FROM game_news n JOIN owned o ON o.id = n.game_id
                  WHERE n.language = ?) AS news_items,
                (SELECT MAX(last_success_at) FROM lang_feeds) AS last_synced_at"
        );

        let (owned_games, synced_games, failed_games, news_items, last_synced_at): (
            i64,
            i64,
            i64,
            i64,
            Option<DateTime<Utc>>,
        ) = sqlx::query_as(&sql)
            .bind(language)
            .bind(language)
            .fetch_one(&self.pool)
            .await
            .db()?;

        Ok(NewsStatus {
            language: language.to_string(),
            owned_games,
            synced_games,
            pending_games: (owned_games - synced_games).max(0),
            failed_games,
            news_items,
            last_synced_at,
        })
    }
}
