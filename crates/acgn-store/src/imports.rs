//! 导入 / 同步：把任意数据源的 [`SourceItem`] 合并进本地库。
//!
//! 合并规则（很重要，决定了「双向同步」的默认行为）：
//!
//! 1. **元数据**（标题、封面、价格、评价……）以来源为准，来源给了值就覆盖；
//!    来源没给值则保留本地已有的值（`None` 不算覆盖）。
//! 2. **个人状态**（`status` / `rating` / `note` / 进度 / 时长）永远以本地为准。
//!    只有当本地状态还是「愿望单 / 待玩」（用户还没表态过）时，才会跟随来源声明的
//!    状态——也就是说「你把某游戏标成在玩/已通关后，再同步愿望单不会把它打回去」。
//!    库存导入据此把“有游玩时长”的条目从愿望单/待玩升级为「在玩」。
//! 3. **游玩时长**是半例外：来源值记录进 `source_playtime_minutes`，而展示值
//!    `playtime_minutes` 只在本地没有手填过（为空或仍等于上次来源值）时跟随来源；
//!    用户手改过就保留本地（见 `plan_merge` 的注释与 `docs/design.md` §4.5）。
//! 4. 来源里消失的条目 **不会被删除**，只统计 `missing` 并给出提示；
//!    `missing` 的候选集按 [`MissingScope`] 区分（愿望单 / 库存）。
//! 5. 每次导入都写一条 `import_runs`，失败也会落库（便于排查）。

use std::collections::HashMap;
use std::time::Instant;

use acgn_core::{
    Error, Game, GameWithEntry, ImportOptions, ImportRun, ImportSummary, LibraryEntry, MediaKind,
    RestoreItem, Result, Source, SourceItem, Status,
};
use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, Sqlite, SqliteConnection, Transaction};
use tracing::{info, warn};

use crate::Db;
use crate::accounts::{aggregate_playtime, load_account_link};
use crate::error::ResultExt;
use crate::rows::{AccountLinkRow, GameEntryRow, ImportRunRow, SELECT_GAME_ENTRY, strings_json};

/// 一次导入请求（由 HTTP 层或 Tauri command 组装）。
#[derive(Debug, Clone)]
pub struct ImportRequest<'a> {
    pub source: Source,
    /// `steam_wishlist` / `json` / `metadata_enrich` / `manual` …
    pub mode: &'a str,
    pub items: &'a [SourceItem],
    pub options: ImportOptions,
    pub args: Option<serde_json::Value>,
    /// 数据实际来源：`live` / `fixture` / `official_api` / `json`
    pub source_mode: &'a str,
    /// 抓取阶段产生的告警（会一并写进运行记录）
    pub extra_warnings: Vec<String>,
    /// `missing` 的统计口径（愿望单导入与库存导入看见的「来源」不同）
    pub missing_scope: MissingScope,
    /// 本次导入关联的数据源账号（Steam 库存 / 愿望单同步）；`None` = 不建立账号关联
    /// （通用 JSON 导入、测试等）。
    pub account_id: Option<i64>,
}

/// `missing`（来源里消失的条目）的候选口径。
///
/// 愿望单导入只对「上次还在愿望单」的条目负责；库存导入只对「上次被库存同步见过」
/// 的条目负责。两者不能混用：拿愿望单的候选集去做库存导入，会把所有未拥有的愿望单
/// 条目误报成 missing，反之亦然。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingScope {
    /// 上次还在愿望单的条目（`steam_wishlist` / JSON 导入）
    #[default]
    Wishlist,
    /// 上次被库存同步见过的条目（以 `library_entries.source_playtime_minutes` 为标记）
    Library,
}

/// 运行记录的计数（供 enrich 等非导入场景复用）。
#[derive(Debug, Clone, Copy, Default)]
pub struct RunCounts {
    pub seen: i64,
    pub added: i64,
    pub updated: i64,
    pub unchanged: i64,
    pub skipped: i64,
    pub missing: i64,
}

/// 单条元数据补全的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataOutcome {
    Added(i64),
    Updated(i64),
    Unchanged(i64),
}

impl MetadataOutcome {
    pub fn game_id(&self) -> i64 {
        match self {
            MetadataOutcome::Added(id)
            | MetadataOutcome::Updated(id)
            | MetadataOutcome::Unchanged(id) => *id,
        }
    }

    /// 供 API 返回的短标签。
    pub fn kind(&self) -> &'static str {
        match self {
            MetadataOutcome::Added(_) => "added",
            MetadataOutcome::Updated(_) => "updated",
            MetadataOutcome::Unchanged(_) => "unchanged",
        }
    }
}

/// 待补全元数据的条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMetadata {
    pub game_id: i64,
    pub external_id: String,
    pub title: String,
    pub source: Source,
}

impl Db {
    // ---------------------------------------------------------------- 运行记录

    /// 开一条运行记录，返回 run_id。
    pub async fn begin_run(
        &self,
        source: Source,
        mode: &str,
        args: Option<serde_json::Value>,
    ) -> Result<i64> {
        let now = Utc::now();
        let result = sqlx::query(
            "INSERT INTO import_runs (source, mode, status, args, started_at) VALUES (?, ?, 'running', ?, ?)",
        )
        .bind(source.as_str())
        .bind(mode)
        .bind(args.map(|value| value.to_string()))
        .bind(now)
        .execute(&self.pool)
        .await
        .db()?;

        Ok(result.last_insert_rowid())
    }

    /// 运行成功收尾。
    pub async fn finish_run_ok(
        &self,
        run_id: i64,
        counts: RunCounts,
        warnings: &[String],
        duration_ms: i64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE import_runs SET status = 'ok', seen = ?, added = ?, updated = ?, unchanged = ?,
                skipped = ?, missing = ?, warnings = ?, finished_at = ?, duration_ms = ?
             WHERE id = ?",
        )
        .bind(counts.seen)
        .bind(counts.added)
        .bind(counts.updated)
        .bind(counts.unchanged)
        .bind(counts.skipped)
        .bind(counts.missing)
        .bind(strings_json(warnings))
        .bind(Utc::now())
        .bind(duration_ms)
        .bind(run_id)
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    /// 运行失败收尾。
    pub async fn finish_run_err(&self, run_id: i64, error: &str, duration_ms: i64) -> Result<()> {
        sqlx::query(
            "UPDATE import_runs SET status = 'failed', error = ?, finished_at = ?, duration_ms = ? WHERE id = ?",
        )
        .bind(error)
        .bind(Utc::now())
        .bind(duration_ms)
        .bind(run_id)
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    /// 最近的运行记录。
    pub async fn list_import_runs(&self, limit: i64) -> Result<Vec<ImportRun>> {
        let rows: Vec<ImportRunRow> =
            sqlx::query_as::<_, ImportRunRow>("SELECT * FROM import_runs ORDER BY id DESC LIMIT ?")
                .bind(limit.clamp(1, 200))
                .fetch_all(&self.pool)
                .await
                .db()?;

        rows.into_iter().map(ImportRunRow::into_domain).collect()
    }

    /// 单条运行记录。
    pub async fn get_import_run(&self, run_id: i64) -> Result<Option<ImportRun>> {
        let row: Option<ImportRunRow> =
            sqlx::query_as::<_, ImportRunRow>("SELECT * FROM import_runs WHERE id = ?")
                .bind(run_id)
                .fetch_optional(&self.pool)
                .await
                .db()?;

        row.map(ImportRunRow::into_domain).transpose()
    }

    // ---------------------------------------------------------------- 导入

    /// 导入一批来源条目（幂等，可重复执行）。
    ///
    /// `dry_run` 只统计、不落库，也不写运行记录（「仅预览」不应污染同步历史）。
    pub async fn import_items(&self, req: &ImportRequest<'_>) -> Result<ImportSummary> {
        let started = Instant::now();

        if req.options.dry_run {
            let mut summary = self.import_items_inner(req).await?;
            summary.duration_ms = started.elapsed().as_millis() as i64;
            return Ok(summary);
        }

        let run_id = self
            .begin_run(req.source, req.mode, req.args.clone())
            .await?;

        match self.import_items_inner(req).await {
            Ok(mut summary) => {
                summary.run_id = run_id;
                summary.duration_ms = started.elapsed().as_millis() as i64;
                self.finish_run_ok(
                    run_id,
                    RunCounts {
                        seen: summary.seen,
                        added: summary.added,
                        updated: summary.updated,
                        unchanged: summary.unchanged,
                        skipped: summary.skipped,
                        missing: summary.missing,
                    },
                    &summary.warnings,
                    summary.duration_ms,
                )
                .await?;

                // 账号的「最近同步」是展示用记账字段：写失败不影响本次导入结果
                if let Some(account_id) = req.account_id
                    && let Err(err) = self.touch_account_synced(account_id, Utc::now()).await
                {
                    warn!(account_id, error = %err, "刷新账号同步时间失败");
                }

                info!(
                    run_id,
                    mode = req.mode,
                    seen = summary.seen,
                    added = summary.added,
                    updated = summary.updated,
                    unchanged = summary.unchanged,
                    skipped = summary.skipped,
                    missing = summary.missing,
                    "导入完成"
                );
                Ok(summary)
            }
            Err(err) => {
                let duration = started.elapsed().as_millis() as i64;
                if let Err(record_err) = self
                    .finish_run_err(run_id, &err.to_string(), duration)
                    .await
                {
                    warn!(run_id, error = %record_err, "写入失败记录时又出错了");
                }
                Err(err)
            }
        }
    }

    async fn import_items_inner(&self, req: &ImportRequest<'_>) -> Result<ImportSummary> {
        let now = Utc::now();

        // 导入前「来源一侧」的条目，用来计算「这次不见了」的 missing。
        // 口径按导入类型区分（见 MissingScope 的说明），不能一套 SQL 用到底。
        // 带账号的导入按 (账号, 愿望单标记) / (账号, 库存标记) 统计：三个账号各自
        // 维护自己的 missing，A 号退款不会把 B 号还在库里的条目报成 missing。
        let previous: Vec<(String, i64)> = match (req.missing_scope, req.account_id) {
            (MissingScope::Wishlist, Some(account_id)) => sqlx::query_as::<_, (String, i64)>(
                "SELECT g.external_id, g.id FROM games g
                 JOIN game_accounts ga ON ga.game_id = g.id
                 WHERE g.source = ? AND ga.account_id = ? AND ga.in_wishlist = 1",
            )
            .bind(req.source.as_str())
            .bind(account_id)
            .fetch_all(&self.pool)
            .await
            .db()?,
            (MissingScope::Library, Some(account_id)) => sqlx::query_as::<_, (String, i64)>(
                "SELECT g.external_id, g.id FROM games g
                 JOIN game_accounts ga ON ga.game_id = g.id
                 WHERE g.source = ? AND ga.account_id = ?
                   AND ga.source_playtime_minutes IS NOT NULL",
            )
            .bind(req.source.as_str())
            .bind(account_id)
            .fetch_all(&self.pool)
            .await
            .db()?,
            (MissingScope::Wishlist, None) => sqlx::query_as::<_, (String, i64)>(
                "SELECT g.external_id, g.id FROM games g
                 JOIN library_entries e ON e.game_id = g.id
                 WHERE g.source = ? AND e.status = 'wishlist'",
            )
            .bind(req.source.as_str())
            .fetch_all(&self.pool)
            .await
            .db()?,
            // 被库存同步见过的标记：source_playtime_minutes 非空（0 也算见过）
            (MissingScope::Library, None) => sqlx::query_as::<_, (String, i64)>(
                "SELECT g.external_id, g.id FROM games g
                 JOIN library_entries e ON e.game_id = g.id
                 WHERE g.source = ? AND e.source_playtime_minutes IS NOT NULL",
            )
            .bind(req.source.as_str())
            .fetch_all(&self.pool)
            .await
            .db()?,
        };
        let mut previous: HashMap<String, i64> = previous.into_iter().collect();

        let mut summary = ImportSummary {
            run_id: 0,
            source: req.source,
            mode: req.mode.to_string(),
            seen: req.items.len() as i64,
            added: 0,
            updated: 0,
            unchanged: 0,
            skipped: 0,
            missing: 0,
            warnings: req.extra_warnings.clone(),
            duration_ms: 0,
            sample_ids: Vec::new(),
            source_mode: req.source_mode.to_string(),
        };

        if req.items.is_empty() {
            summary
                .warnings
                .push("来源返回 0 个条目：请检查 SteamID、隐私设置或数据源模式".to_string());
        }

        if req.options.dry_run {
            // 预览与真实导入共用同一套差异计算：报「会更新 N 条」就真的是 N 条，
            // 而不是「来源里有 + 本地已存在」这种口径不一致的估算。
            // 内存库只有一条连接：预览不开事务，但复用同一条连接做账号聚合。
            let mut conn = self.pool.acquire().await.db()?;
            let conn = &mut *conn;
            for item in req.items {
                // 先记下「来源里确实还有它」——被 include_dlc 之类的过滤跳过的条目
                // 仍然存在于来源中，不能算进 missing。
                previous.remove(&item.external_id);
                if !accepted(item, &req.options) {
                    summary.skipped += 1;
                    continue;
                }
                match load_row(&mut *conn, item.source, &item.external_id).await? {
                    Some(row) => {
                        let (account_changed, playtime) = match req.account_id {
                            Some(account_id) => {
                                plan_account_link(conn, row.id, account_id, item, req.missing_scope)
                                    .await?
                            }
                            None => (false, item.playtime_minutes),
                        };
                        let plan = plan_merge(&row, item, playtime, now)?;
                        if plan.changed() || account_changed {
                            summary.updated += 1;
                        } else {
                            summary.unchanged += 1;
                        }
                    }
                    None => summary.added += 1,
                }
            }
            summary.missing = previous.len() as i64;
            push_missing_warning(&mut summary, &previous, req.missing_scope);
            return Ok(summary);
        }

        let mut tx = self.pool.begin().await.db()?;
        for item in req.items {
            // 同上：过滤只影响「是否落库」，不影响「来源里是否还有它」
            previous.remove(&item.external_id);
            if !accepted(item, &req.options) {
                summary.skipped += 1;
                continue;
            }

            match upsert_item(&mut tx, item, req.account_id, req.missing_scope, now).await? {
                MetadataOutcome::Added(id) => {
                    summary.added += 1;
                    push_sample(&mut summary.sample_ids, id);
                }
                MetadataOutcome::Updated(id) => {
                    summary.updated += 1;
                    push_sample(&mut summary.sample_ids, id);
                }
                MetadataOutcome::Unchanged(_) => summary.unchanged += 1,
            }
        }
        tx.commit().await.db()?;

        push_missing_warning(&mut summary, &previous, req.missing_scope);
        summary.sample_ids.truncate(24);
        summary.sample_ids.sort_unstable();
        summary.sample_ids.dedup();

        Ok(summary)
    }

    // ---------------------------------------------------------------- 备份还原

    /// 备份还原：把导出文件里的条目写回（**文件优先**，刻意不走合并语义）。
    ///
    /// 与 [`Db::import_items`] 的区别：
    ///
    /// - 元数据与个人状态（状态 / 评分 / 备注 / 进度 / 时长 / 收藏 / 优先级）都以文件为准；
    /// - `entry` 缺失时按「愿望单、无个人状态」创建；
    /// - 文件里没有的条目不会被删除；
    /// - `metadata_json`（原始载荷缓存）不在导出范围内，保持原值；
    /// - `created_at` / `updated_at` / `last_synced_at` 属于记账字段，会刷新为当前时间
    ///   （因此还原不是逐字节相等，但「元数据 + 个人状态」是可逆的）。
    ///
    /// `dry_run` 只统计不落库（`run_id == 0`，也不写运行记录）。
    pub async fn restore_items(
        &self,
        items: &[RestoreItem],
        dry_run: bool,
    ) -> Result<ImportSummary> {
        let started = Instant::now();
        let now = Utc::now();

        let mut sources: HashMap<String, i64> = HashMap::new();
        for item in items {
            *sources
                .entry(item.game.source.as_str().to_string())
                .or_default() += 1;
        }

        let mut summary = ImportSummary {
            run_id: 0,
            source: Source::Manual,
            mode: "restore".to_string(),
            seen: items.len() as i64,
            added: 0,
            updated: 0,
            unchanged: 0,
            skipped: 0,
            missing: 0,
            warnings: Vec::new(),
            duration_ms: 0,
            sample_ids: Vec::new(),
            source_mode: "json".to_string(),
        };

        if items.is_empty() {
            summary
                .warnings
                .push("items 为空：导出文件里没有可还原的条目".to_string());
            summary.duration_ms = started.elapsed().as_millis() as i64;
            return Ok(summary);
        }

        let run_id = if dry_run {
            0
        } else {
            self.begin_run(
                Source::Manual,
                "restore",
                Some(serde_json::json!({ "items": items.len(), "sources": sources })),
            )
            .await?
        };

        match self
            .restore_items_inner(items, dry_run, &mut summary, now)
            .await
        {
            Ok(()) => {
                summary.duration_ms = started.elapsed().as_millis() as i64;
                if !dry_run {
                    self.finish_run_ok(
                        run_id,
                        RunCounts {
                            seen: summary.seen,
                            added: summary.added,
                            updated: summary.updated,
                            unchanged: summary.unchanged,
                            skipped: summary.skipped,
                            missing: 0,
                        },
                        &summary.warnings,
                        summary.duration_ms,
                    )
                    .await?;
                    summary.run_id = run_id;
                }
                info!(
                    run_id,
                    dry_run,
                    added = summary.added,
                    updated = summary.updated,
                    unchanged = summary.unchanged,
                    "备份还原完成"
                );
                Ok(summary)
            }
            Err(err) => {
                if !dry_run {
                    let duration = started.elapsed().as_millis() as i64;
                    if let Err(record_err) = self
                        .finish_run_err(run_id, &err.to_string(), duration)
                        .await
                    {
                        warn!(run_id, error = %record_err, "写入失败记录时又出错了");
                    }
                }
                Err(err)
            }
        }
    }

    async fn restore_items_inner(
        &self,
        items: &[RestoreItem],
        dry_run: bool,
        summary: &mut ImportSummary,
        now: DateTime<Utc>,
    ) -> Result<()> {
        // 预览不落库，也不开事务（内存库只有一条连接，事务会占住它）
        if dry_run {
            for item in items {
                let Some(external_id) = valid_external_id(item) else {
                    summary.skipped += 1;
                    continue;
                };
                let previous = load_row(&self.pool, item.game.source, external_id)
                    .await?
                    .map(GameEntryRow::into_domain)
                    .transpose()?;
                match plan_restore(previous, item, now)?.2 {
                    RestoreAction::Added => summary.added += 1,
                    RestoreAction::Updated => summary.updated += 1,
                    RestoreAction::Unchanged => summary.unchanged += 1,
                }
            }
            return Ok(());
        }

        let mut tx = self.pool.begin().await.db()?;
        for item in items {
            let Some(external_id) = valid_external_id(item) else {
                summary.skipped += 1;
                summary
                    .warnings
                    .push("有 1 条缺少 external_id，已跳过".to_string());
                continue;
            };

            let previous = load_in_tx(&mut tx, item.game.source, external_id)
                .await?
                .map(GameEntryRow::into_domain)
                .transpose()?;
            let (game, entry, action) = plan_restore(previous, item, now)?;

            match action {
                RestoreAction::Added => {
                    let game_id = insert_game_in_tx(&mut tx, &game, None, now).await?;
                    let mut entry = entry;
                    entry.game_id = game_id;
                    entry.created_at = now;
                    entry.updated_at = now;
                    entry.last_synced_at = Some(now);
                    insert_entry_in_tx(&mut tx, &entry).await?;
                    summary.added += 1;
                    push_sample(&mut summary.sample_ids, game_id);
                }
                RestoreAction::Updated => {
                    update_game_in_tx(&mut tx, &game, now).await?;
                    update_entry_in_tx(&mut tx, &entry).await?;
                    summary.updated += 1;
                    push_sample(&mut summary.sample_ids, game.id);
                }
                RestoreAction::Unchanged => {
                    summary.unchanged += 1;
                    sqlx::query("UPDATE library_entries SET last_synced_at = ? WHERE game_id = ?")
                        .bind(now)
                        .bind(game.id)
                        .execute(&mut *tx)
                        .await
                        .db()?;
                }
            }
        }
        tx.commit().await.db()?;

        summary.sample_ids.truncate(24);
        summary.sample_ids.sort_unstable();
        summary.sample_ids.dedup();
        Ok(())
    }

    // ---------------------------------------------------------------- 元数据补全

    /// 用完整元数据（Steam appdetails 等）更新单条；不存在则新建。
    pub async fn update_metadata(&self, item: &SourceItem) -> Result<MetadataOutcome> {
        let now = Utc::now();
        let mut tx = self.pool.begin().await.db()?;

        let outcome = match load_in_tx(&mut tx, item.source, &item.external_id).await? {
            Some(row) => {
                let metadata_json_changed = metadata_json_differs(&row, item);
                let mut game = row.into_domain()?.game;
                let changed = merge_metadata(&mut game, item);
                let had_metadata = game.metadata_updated_at.is_some();
                let any_change = changed || metadata_json_changed || !had_metadata;

                // 没有任何变化时不写 games：否则反复点「刷新元数据」会让
                // updated_at / metadata_updated_at 一直漂移，看起来像内容变了。
                if any_change {
                    game.metadata_updated_at = Some(now);
                    update_game_in_tx(&mut tx, &game, now).await?;
                    if metadata_json_changed {
                        write_metadata_json_in_tx(&mut tx, game.id, item, now).await?;
                    }
                }

                sqlx::query("UPDATE library_entries SET last_synced_at = ? WHERE game_id = ?")
                    .bind(now)
                    .bind(game.id)
                    .execute(&mut *tx)
                    .await
                    .db()?;

                if any_change {
                    MetadataOutcome::Updated(game.id)
                } else {
                    MetadataOutcome::Unchanged(game.id)
                }
            }
            None => {
                if let Some(game_id) =
                    game_id_in_tx(&mut tx, item.source, &item.external_id).await?
                {
                    // 游戏行在但关系行丢了：补一条默认关系行，避免下面插入撞唯一索引。
                    ensure_entry_in_tx(&mut tx, game_id, &Status::Wishlist, Some(now), now).await?;
                    let row = load_in_tx(&mut tx, item.source, &item.external_id)
                        .await?
                        .ok_or_else(|| Error::storage("关系行修复失败"))?;
                    // 修好关系行后走与主分支相同的差异判定：病态库里如果元数据本来就
                    // 一致，只刷新 last_synced_at，不要让 updated_at 一直漂移。
                    let metadata_json_changed = metadata_json_differs(&row, item);
                    let mut game = row.into_domain()?.game;
                    let changed = merge_metadata(&mut game, item);
                    let had_metadata = game.metadata_updated_at.is_some();
                    let any_change = changed || metadata_json_changed || !had_metadata;
                    if any_change {
                        game.metadata_updated_at = Some(now);
                        update_game_in_tx(&mut tx, &game, now).await?;
                        if metadata_json_changed {
                            write_metadata_json_in_tx(&mut tx, game.id, item, now).await?;
                        }
                    }
                    sqlx::query("UPDATE library_entries SET last_synced_at = ? WHERE game_id = ?")
                        .bind(now)
                        .bind(game.id)
                        .execute(&mut *tx)
                        .await
                        .db()?;
                    if any_change {
                        MetadataOutcome::Updated(game.id)
                    } else {
                        MetadataOutcome::Unchanged(game.id)
                    }
                } else {
                    let game = game_from_item(item, now, true);
                    let game_id =
                        insert_game_in_tx(&mut tx, &game, metadata_to_text(item), now).await?;
                    let mut entry = new_entry(game_id, item, now);
                    entry.last_synced_at = Some(now);
                    insert_entry_in_tx(&mut tx, &entry).await?;
                    MetadataOutcome::Added(game_id)
                }
            }
        };

        tx.commit().await.db()?;
        Ok(outcome)
    }

    /// 取出还没抓过完整元数据的条目（愿望单优先）。
    pub async fn pending_metadata(
        &self,
        source: Option<Source>,
        limit: i64,
    ) -> Result<Vec<PendingMetadata>> {
        let mut qb = QueryBuilder::new(
            "SELECT g.id AS game_id, g.external_id AS external_id, g.title AS title, g.source AS source
             FROM games g JOIN library_entries e ON e.game_id = g.id
             WHERE g.metadata_updated_at IS NULL",
        );
        if let Some(source) = source {
            qb.push(" AND g.source = ").push_bind(source.as_str());
        }
        qb.push(
            " ORDER BY CASE e.status WHEN 'wishlist' THEN 0 WHEN 'playing' THEN 1 ELSE 2 END,
               g.review_score IS NULL, g.review_score DESC, g.id ASC LIMIT ",
        )
        .push_bind(limit.clamp(1, 500));

        let rows: Vec<(i64, String, String, String)> =
            qb.build_query_as().fetch_all(&self.pool).await.db()?;

        rows.into_iter()
            .map(|(game_id, external_id, title, source)| {
                let source = Source::from_str_lossy(&source)
                    .ok_or_else(|| Error::storage(format!("非法 source: {source}")))?;
                Ok(PendingMetadata {
                    game_id,
                    external_id,
                    title,
                    source,
                })
            })
            .collect()
    }

    /// 读取若干条详情（导入结果预览用）。
    pub async fn load_many(&self, ids: &[i64]) -> Result<Vec<GameWithEntry>> {
        let mut result = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(item) = self.get_game(*id).await? {
                result.push(item);
            }
        }
        Ok(result)
    }
}

// -------------------------------------------------------------------- 内部实现

/// 还原单条的结果归类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreAction {
    Added,
    Updated,
    Unchanged,
}

/// 合法的 `external_id`：空串或纯空白直接跳过。
fn valid_external_id(item: &RestoreItem) -> Option<&str> {
    let external_id = item.game.external_id.trim();
    (!external_id.is_empty()).then_some(external_id)
}

/// 规划单条还原：算出要写入的 `(game, entry)` 以及「相对本地有没有变化」。
fn plan_restore(
    previous: Option<GameWithEntry>,
    item: &RestoreItem,
    now: DateTime<Utc>,
) -> Result<(Game, LibraryEntry, RestoreAction)> {
    let mut game = item.game.clone();
    let mut entry = item.entry.clone().unwrap_or(LibraryEntry {
        game_id: 0,
        status: Status::Wishlist,
        priority: None,
        favorite: false,
        rating: None,
        progress_percent: None,
        playtime_minutes: None,
        source_playtime_minutes: None,
        achievements_achieved: None,
        achievements_total: None,
        achievements_checked_at: None,
        note: None,
        source_added_at: None,
        started_at: None,
        finished_at: None,
        last_synced_at: None,
        created_at: now,
        updated_at: now,
    });

    match previous {
        None => {
            game.id = 0;
            game.created_at = now;
            game.updated_at = now;
            entry.game_id = 0;
            entry.created_at = now;
            entry.updated_at = now;
            entry.last_synced_at = Some(now);
            Ok((game, entry, RestoreAction::Added))
        }
        Some(previous) => {
            let game_id = previous.game.id;
            game.id = game_id;
            // created_at / updated_at 是记账字段：保留本地值、不参与「有没有变化」比较
            game.created_at = previous.game.created_at;
            game.updated_at = now;
            entry.game_id = game_id;
            entry.created_at = previous.entry.created_at;
            entry.updated_at = now;
            entry.last_synced_at = Some(now);

            let changed = restore_changed(&previous, &game, &entry);
            Ok((
                game,
                entry,
                if changed {
                    RestoreAction::Updated
                } else {
                    RestoreAction::Unchanged
                },
            ))
        }
    }
}

/// 还原时的「有没有变化」判定：忽略 id 与纯记账时间戳。
fn restore_changed(previous: &GameWithEntry, game: &Game, entry: &LibraryEntry) -> bool {
    let mut local_game = previous.game.clone();
    local_game.id = game.id;
    local_game.created_at = game.created_at;
    local_game.updated_at = game.updated_at;

    let mut local_entry = previous.entry.clone();
    local_entry.game_id = entry.game_id;
    local_entry.created_at = entry.created_at;
    local_entry.updated_at = entry.updated_at;
    local_entry.last_synced_at = entry.last_synced_at;

    local_game != *game || local_entry != *entry
}

/// 「来源里消失了」的告警：预览与真实导入共用同一段文案，避免两边说法不一致。
fn push_missing_warning(
    summary: &mut ImportSummary,
    previous: &HashMap<String, i64>,
    scope: MissingScope,
) {
    summary.missing = previous.len() as i64;
    if summary.missing == 0 {
        return;
    }
    let preview: Vec<String> = previous.values().take(5).map(|id| id.to_string()).collect();
    let reason = match scope {
        MissingScope::Wishlist => "（可能已购买 / 被移出愿望单）",
        MissingScope::Library => "（可能已退款 / 移出库存 / 家庭共享失效）",
    };
    summary.warnings.push(format!(
        "{} 个条目已不在来源中{reason}，本地保留未删除：#{}",
        summary.missing,
        preview.join(", #")
    ));
}

fn accepted(item: &SourceItem, options: &ImportOptions) -> bool {
    match item.media_kind {
        MediaKind::Game => true,
        MediaKind::Dlc => options.include_dlc,
        _ => options.include_non_games,
    }
}

fn push_sample(sample_ids: &mut Vec<i64>, id: i64) {
    if sample_ids.len() < 24 {
        sample_ids.push(id);
    }
}

/// 新条目落库（`account_id` 非空时同时建立账号关联）。
async fn upsert_item(
    tx: &mut Transaction<'_, Sqlite>,
    item: &SourceItem,
    account_id: Option<i64>,
    scope: MissingScope,
    now: DateTime<Utc>,
) -> Result<MetadataOutcome> {
    if let Some(row) = load_in_tx(tx, item.source, &item.external_id).await? {
        return merge_existing(tx, row, item, account_id, scope, now).await;
    }

    if let Some(game_id) = game_id_in_tx(tx, item.source, &item.external_id).await? {
        ensure_entry_in_tx(tx, game_id, &Status::Wishlist, item.source_added_at, now).await?;
        let row = load_in_tx(tx, item.source, &item.external_id)
            .await?
            .ok_or_else(|| Error::storage("关系行修复失败"))?;
        return merge_existing(tx, row, item, account_id, scope, now).await;
    }

    let game = game_from_item(item, now, false);
    let game_id = insert_game_in_tx(tx, &game, metadata_to_text(item), now).await?;
    let entry = new_entry(game_id, item, now);
    insert_entry_in_tx(tx, &entry).await?;
    if let Some(account_id) = account_id {
        upsert_account_link(tx, game_id, account_id, item, scope, now).await?;
    }

    Ok(MetadataOutcome::Added(game_id))
}

/// 一次合并的「计划」：只算差异，不落库。
///
/// 真实导入与 `dry_run` 预览共用它，保证两者口径一致（预览报「会更新 N 条」必须是
/// 真的会写 N 条，而不是「来源里有 + 本地已存在」）。
struct MergePlan {
    game: Game,
    entry: LibraryEntry,
    metadata_changed: bool,
    metadata_json_changed: bool,
    entry_changed: bool,
}

impl MergePlan {
    fn changed(&self) -> bool {
        self.metadata_changed || self.metadata_json_changed || self.entry_changed
    }
}

/// 把来源条目合并到已有行上（纯计算）。
///
/// `playtime_minutes` 是本次要写入「来源跟踪值」的时长：
/// * 不关联账号时 = 条目自己的 `playtime_minutes`（历史行为）；
/// * 关联账号时 = 该游戏所有账号时长的聚合（`MAX`，见 [`aggregate_playtime`]）——
///   这样小号（0 小时）后同步也不会把主号的时长覆盖掉。
fn plan_merge(
    row: &GameEntryRow,
    item: &SourceItem,
    playtime_minutes: Option<i64>,
    now: DateTime<Utc>,
) -> Result<MergePlan> {
    let GameWithEntry {
        mut game,
        mut entry,
        ..
    } = row.clone().into_domain()?;

    let metadata_changed = merge_metadata(&mut game, item);
    let metadata_json_changed = metadata_json_differs(row, item);

    let mut entry_changed = false;
    // SQLite 的时间戳只有微秒精度，来源可能给到纳秒：必须按微秒比较，
    // 否则每次同步都会因为「精度差」误判成有变化。
    if let Some(added) = item.source_added_at
        && !same_instant(entry.source_added_at, Some(added))
    {
        entry.source_added_at = Some(added);
        entry_changed = true;
    }
    if let Some(priority) = item.priority
        && entry.priority != Some(priority)
    {
        entry.priority = Some(priority);
        entry_changed = true;
    }
    // 本地状态是权威：只有「用户还没表态过」的两档（愿望单 / 待玩）才跟随来源，
    // 且来源不能把条目打回愿望单。用户手动标过的在玩/已通关/搁置/弃坑/忽略永不回退。
    //
    // 库存导入靠这条把「有游玩时长」的已拥有游戏升级为在玩：适配器给出
    // `status = playing`，仍在愿望单/待玩的条目会跟随，已通关之类的则保持本地值。
    if matches!(entry.status, Status::Wishlist | Status::Backlog)
        && let Some(status) = item.status
        && status != Status::Wishlist
        && entry.status != status
    {
        entry.status = status;
        // 与手动 PATCH 对齐：切到在玩/已通关时补时间戳（否则「在玩」没有开始时间）
        apply_status_side_effects(&mut entry, now);
        entry_changed = true;
    }

    // 游玩时长：来源值是「测量结果」，但本地手填的值优先。
    //
    // * `source_playtime_minutes` 始终记录来源报告的值（含 0），用于展示与判定；
    // * `playtime_minutes`（展示 / 统计用）在「本地没手改过」时跟随来源——为空，
    //   或仍等于上次来源值；一旦两者不同（用户手填 / 其它平台时长）就保留本地，
    //   直到用户清空或用「采用 Steam 时长」恢复跟随。
    //
    // 多账号时这里拿到的是聚合值（MAX）：只有聚合值变化才算变化。
    if let Some(playtime) = playtime_minutes {
        let previous_source = entry.source_playtime_minutes;
        if previous_source != Some(playtime) {
            entry.source_playtime_minutes = Some(playtime);
            entry_changed = true;
        }
        if (entry.playtime_minutes.is_none() || entry.playtime_minutes == previous_source)
            && entry.playtime_minutes != Some(playtime)
        {
            entry.playtime_minutes = Some(playtime);
            entry_changed = true;
        }
    }

    if entry_changed {
        entry.updated_at = now;
        entry.last_synced_at = Some(now);
    }

    Ok(MergePlan {
        game,
        entry,
        metadata_changed,
        metadata_json_changed,
        entry_changed,
    })
}

/// 状态切换的附带写入（与手动 PATCH 同一套语义）：
/// 在玩 → 补开始时间、清完成时间；已通关 → 补开始/完成时间与 100% 进度。
/// 导入收养（[`plan_merge`]）与 `Db::update_entry` 共用，避免两条路语义漂移。
pub(crate) fn apply_status_side_effects(entry: &mut LibraryEntry, now: DateTime<Utc>) {
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

/// `metadata_json` 的比较。
///
/// 与封面 / 发售日同一条规则（AGENTS.md §4.2）：列表档（愿望单载荷）只允许填空，
/// 详情档（appdetails 载荷）才能覆盖。否则「导入 → 补全 → 再导入」会让两种载荷
/// 互相改写，每次同步都误报「有更新」。
fn metadata_json_differs(row: &GameEntryRow, item: &SourceItem) -> bool {
    let Some(incoming) = &item.metadata else {
        return false;
    };

    if !item.metadata_level.is_full() && row.metadata_json.is_some() {
        return false;
    }

    match row
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
    {
        Some(existing) => existing != *incoming,
        None => true,
    }
}

fn metadata_to_text(item: &SourceItem) -> Option<String> {
    item.metadata.as_ref().map(|value| value.to_string())
}

async fn merge_existing(
    tx: &mut Transaction<'_, Sqlite>,
    row: GameEntryRow,
    item: &SourceItem,
    account_id: Option<i64>,
    scope: MissingScope,
    now: DateTime<Utc>,
) -> Result<MetadataOutcome> {
    // 账号关联先落库，再按聚合值规划条目：真实导入与 dry_run 都走同一套聚合口径。
    let (account_changed, playtime) = match account_id {
        Some(account_id) => {
            let changed = upsert_account_link(tx, row.id, account_id, item, scope, now).await?;
            let aggregate = aggregate_playtime(&mut *tx, row.id, None).await?;
            (changed, aggregate)
        }
        None => (false, item.playtime_minutes),
    };

    let plan = plan_merge(&row, item, playtime, now)?;

    if plan.metadata_changed {
        update_game_in_tx(tx, &plan.game, now).await?;
    }

    if plan.metadata_json_changed {
        write_metadata_json_in_tx(tx, plan.game.id, item, now).await?;
    }

    if plan.entry_changed {
        update_entry_in_tx(tx, &plan.entry).await?;
    } else {
        sqlx::query("UPDATE library_entries SET last_synced_at = ? WHERE game_id = ?")
            .bind(now)
            .bind(plan.game.id)
            .execute(&mut **tx)
            .await
            .db()?;
    }

    if plan.changed() || account_changed {
        Ok(MetadataOutcome::Updated(plan.game.id))
    } else {
        Ok(MetadataOutcome::Unchanged(plan.game.id))
    }
}

/// 账号关联的「计划值」：导入只覆盖来源真的给了值的字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlannedLink {
    in_wishlist: bool,
    source_playtime_minutes: Option<i64>,
    playtime_2weeks_minutes: Option<i64>,
    last_played_at: Option<DateTime<Utc>>,
}

/// 愿望单导入把 `in_wishlist` 置 1；库存导入置 0（Steam 买到手后会移出愿望单）。
/// 其它导入类型（理论上不会带账号）保持原值。
fn planned_link(
    existing: Option<&AccountLinkRow>,
    item: &SourceItem,
    scope: MissingScope,
) -> PlannedLink {
    let previous = existing.cloned().unwrap_or(AccountLinkRow {
        in_wishlist: false,
        source_playtime_minutes: None,
        playtime_2weeks_minutes: None,
        last_played_at: None,
    });

    PlannedLink {
        in_wishlist: match scope {
            MissingScope::Wishlist => true,
            MissingScope::Library => false,
        },
        source_playtime_minutes: item.playtime_minutes.or(previous.source_playtime_minutes),
        playtime_2weeks_minutes: item
            .playtime_2weeks_minutes
            .or(previous.playtime_2weeks_minutes),
        last_played_at: item.last_played_at.or(previous.last_played_at),
    }
}

/// 预览用：算出「关联是否有变化」以及聚合后的来源时长，不落库。
async fn plan_account_link(
    conn: &mut SqliteConnection,
    game_id: i64,
    account_id: i64,
    item: &SourceItem,
    scope: MissingScope,
) -> Result<(bool, Option<i64>)> {
    let existing = load_account_link(conn, game_id, account_id).await?;
    let planned = planned_link(existing.as_ref(), item, scope);
    let changed = match existing.as_ref() {
        Some(previous) => {
            planned.in_wishlist != previous.in_wishlist
                || planned.source_playtime_minutes != previous.source_playtime_minutes
                || planned.playtime_2weeks_minutes != previous.playtime_2weeks_minutes
                || !same_instant(planned.last_played_at, previous.last_played_at)
        }
        None => true,
    };

    // 与真实路径一致：库存导入用「其它账号的最大值 + 本次值」，愿望单导入只读聚合。
    let aggregate = match item.playtime_minutes {
        Some(playtime) => {
            let others = aggregate_playtime(conn, game_id, Some(account_id)).await?;
            max_playtime(others, Some(playtime))
        }
        None => aggregate_playtime(conn, game_id, None).await?,
    };

    Ok((changed, aggregate))
}

/// 写入账号关联：`last_synced_at` 每次都推进（记账），`updated_at` 只在内容真的变化时推进。
async fn upsert_account_link(
    tx: &mut Transaction<'_, Sqlite>,
    game_id: i64,
    account_id: i64,
    item: &SourceItem,
    scope: MissingScope,
    now: DateTime<Utc>,
) -> Result<bool> {
    let existing = load_account_link(&mut *tx, game_id, account_id).await?;
    let planned = planned_link(existing.as_ref(), item, scope);

    match existing {
        None => {
            sqlx::query(
                "INSERT INTO game_accounts (
                    game_id, account_id, in_wishlist, source_playtime_minutes,
                    playtime_2weeks_minutes, last_played_at, last_synced_at, created_at, updated_at
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(game_id)
            .bind(account_id)
            .bind(planned.in_wishlist)
            .bind(planned.source_playtime_minutes)
            .bind(planned.playtime_2weeks_minutes)
            .bind(planned.last_played_at)
            .bind(now)
            .bind(now)
            .bind(now)
            .execute(&mut **tx)
            .await
            .db()?;
            Ok(true)
        }
        Some(previous) => {
            let changed = planned.in_wishlist != previous.in_wishlist
                || planned.source_playtime_minutes != previous.source_playtime_minutes
                || planned.playtime_2weeks_minutes != previous.playtime_2weeks_minutes
                || !same_instant(planned.last_played_at, previous.last_played_at);

            if changed {
                sqlx::query(
                    "UPDATE game_accounts SET
                        in_wishlist = ?, source_playtime_minutes = ?, playtime_2weeks_minutes = ?,
                        last_played_at = ?, last_synced_at = ?, updated_at = ?
                     WHERE game_id = ? AND account_id = ?",
                )
                .bind(planned.in_wishlist)
                .bind(planned.source_playtime_minutes)
                .bind(planned.playtime_2weeks_minutes)
                .bind(planned.last_played_at)
                .bind(now)
                .bind(now)
                .bind(game_id)
                .bind(account_id)
                .execute(&mut **tx)
                .await
                .db()?;
            } else {
                sqlx::query(
                    "UPDATE game_accounts SET last_synced_at = ?
                     WHERE game_id = ? AND account_id = ?",
                )
                .bind(now)
                .bind(game_id)
                .bind(account_id)
                .execute(&mut **tx)
                .await
                .db()?;
            }
            Ok(changed)
        }
    }
}

/// 两个可选时长取较大者（忽略 `None`）。
fn max_playtime(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (left, right) => left.or(right),
    }
}

/// 元数据合并：来源给值就覆盖，没给就保留本地。
///
/// 例外：**详情档更权威的字段**（封面、海报、发售日期文案）只允许「完整档」覆盖已有值。
/// 理由：列表档（愿望单）只有通用封面 URL 和英文日期文案，详情档（appdetails）有带 hash
/// 的正式封面和本地化文案；让列表档覆盖会造成「导入 → 补全 → 再导入」来回改写，
/// 每次同步都误报「有更新」。
fn merge_metadata(game: &mut Game, item: &SourceItem) -> bool {
    let mut changed = false;
    let full = item.metadata_level.is_full();

    changed |= merge_title(game, item);
    changed |= take_opt(&mut game.title_zh, item.title_zh.as_ref());
    changed |= take_opt(&mut game.short_description, item.short_description.as_ref());
    changed |= take_authoritative(&mut game.cover_url, item.cover_url.as_ref(), full);
    changed |= take_authoritative(&mut game.poster_url, item.poster_url.as_ref(), full);
    changed |= take_authoritative(&mut game.release_raw, item.release_raw.as_ref(), full);
    changed |= take_authoritative(&mut game.release_date, item.release_date.as_ref(), full);
    changed |= take_opt(&mut game.review_score, item.review_score.as_ref());
    changed |= take_opt(&mut game.review_desc, item.review_desc.as_ref());
    changed |= take_opt(&mut game.review_count, item.review_count.as_ref());
    changed |= take_opt(&mut game.is_free, item.is_free.as_ref());
    changed |= take_opt(&mut game.price_currency, item.price_currency.as_ref());
    changed |= take_opt(&mut game.price_initial, item.price_initial.as_ref());
    changed |= take_opt(&mut game.price_final, item.price_final.as_ref());
    changed |= take_opt(&mut game.discount_percent, item.discount_percent.as_ref());
    changed |= take_vec(&mut game.developers, &item.developers);
    changed |= take_vec(&mut game.publishers, &item.publishers);
    changed |= take_vec(&mut game.genres, &item.genres);
    changed |= take_vec(&mut game.platforms, &item.platforms);
    changed |= take_vec(&mut game.tags, &item.tags);

    if item.media_kind != MediaKind::Other && item.media_kind != game.media_kind {
        game.media_kind = item.media_kind;
        changed = true;
    }

    changed
}

/// 标题合并：忽略 `™` / `®` / 大小写 / 多余空格的差异，避免每次同步都判定为「有变化」。
fn merge_title(game: &mut Game, item: &SourceItem) -> bool {
    let incoming = item.title.trim();
    if incoming.is_empty() {
        return false;
    }
    if normalize_title(&game.title) == normalize_title(incoming) {
        return false;
    }
    game.title = incoming.to_string();
    true
}

fn normalize_title(value: &str) -> String {
    value
        .chars()
        .filter(|c| !matches!(c, '™' | '®' | '©' | '\u{200b}'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// 两个时间戳是否表示同一时刻（按微秒比较，兼容存储精度损失）。
fn same_instant(left: Option<DateTime<Utc>>, right: Option<DateTime<Utc>>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.timestamp_micros() == right.timestamp_micros(),
        (None, None) => true,
        _ => false,
    }
}

fn take_opt<T: PartialEq + Clone>(slot: &mut Option<T>, incoming: Option<&T>) -> bool {
    match incoming {
        Some(value) if slot.as_ref() != Some(value) => {
            *slot = Some(value.clone());
            true
        }
        _ => false,
    }
}

/// 「详情档更权威」的字段合并：`full`（详情档）可以覆盖，列表档只能填空。
fn take_authoritative(slot: &mut Option<String>, incoming: Option<&String>, full: bool) -> bool {
    let Some(value) = incoming
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    if slot.as_deref() == Some(value) {
        return false;
    }
    if !full && slot.is_some() {
        return false;
    }
    *slot = Some(value.to_string());
    true
}

fn take_vec(slot: &mut Vec<String>, incoming: &[String]) -> bool {
    if incoming.is_empty() || slot.as_slice() == incoming {
        false
    } else {
        *slot = incoming.to_vec();
        true
    }
}

fn game_from_item(item: &SourceItem, now: DateTime<Utc>, metadata_stamp: bool) -> Game {
    Game {
        id: 0,
        source: item.source,
        external_id: item.external_id.clone(),
        media_kind: item.media_kind,
        title: if !item.title.trim().is_empty() {
            item.title.trim().to_string()
        } else if let Some(zh) = item.title_zh.as_deref().filter(|zh| !zh.trim().is_empty()) {
            zh.trim().to_string()
        } else {
            format!("{} #{}", item.source.label(), item.external_id)
        },
        title_zh: item.title_zh.clone(),
        short_description: item.short_description.clone(),
        cover_url: item.cover_url.clone(),
        poster_url: item.poster_url.clone(),
        release_date: item.release_date.clone(),
        release_raw: item.release_raw.clone(),
        review_score: item.review_score,
        review_desc: item.review_desc.clone(),
        review_count: item.review_count,
        is_free: item.is_free,
        price_currency: item.price_currency.clone(),
        price_initial: item.price_initial,
        price_final: item.price_final,
        discount_percent: item.discount_percent,
        developers: item.developers.clone(),
        publishers: item.publishers.clone(),
        genres: item.genres.clone(),
        platforms: item.platforms.clone(),
        tags: item.tags.clone(),
        metadata_updated_at: metadata_stamp.then_some(now),
        created_at: now,
        updated_at: now,
    }
}

fn new_entry(game_id: i64, item: &SourceItem, now: DateTime<Utc>) -> LibraryEntry {
    let mut entry = LibraryEntry {
        game_id,
        status: item.status.unwrap_or(Status::Wishlist),
        priority: item.priority,
        favorite: false,
        rating: None,
        progress_percent: None,
        // 新条目尚不存在「本地手填值」：有效时长直接跟随来源
        playtime_minutes: item.playtime_minutes,
        source_playtime_minutes: item.playtime_minutes,
        // 成就进度由独立的成就同步写入，导入时保持未知
        achievements_achieved: None,
        achievements_total: None,
        achievements_checked_at: None,
        note: None,
        source_added_at: item.source_added_at,
        started_at: None,
        finished_at: None,
        last_synced_at: Some(now),
        created_at: now,
        updated_at: now,
    };
    // 与手动 PATCH / 收养路径共用同一套附带写入：首次落库就带「在玩 / 已通关」
    // 的条目也要有开始时间（JSON 导入声明 completed 时同理补完成时间与进度）
    apply_status_side_effects(&mut entry, now);
    entry
}

async fn load_in_tx(
    conn: &mut SqliteConnection,
    source: Source,
    external_id: &str,
) -> Result<Option<GameEntryRow>> {
    load_row(&mut *conn, source, external_id).await
}

/// 通用读取：连接池（预览）与事务连接都能用。
async fn load_row<'e, E>(
    executor: E,
    source: Source,
    external_id: &str,
) -> Result<Option<GameEntryRow>>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let mut qb = QueryBuilder::new(SELECT_GAME_ENTRY);
    qb.push(" WHERE g.source = ")
        .push_bind(source.as_str())
        .push(" AND g.external_id = ")
        .push_bind(external_id);

    qb.build_query_as().fetch_optional(executor).await.db()
}

/// 写入来源原始载荷（`games.metadata_json`）。
async fn write_metadata_json_in_tx(
    conn: &mut SqliteConnection,
    game_id: i64,
    item: &SourceItem,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query("UPDATE games SET metadata_json = ?, updated_at = ? WHERE id = ?")
        .bind(metadata_to_text(item))
        .bind(now)
        .bind(game_id)
        .execute(&mut *conn)
        .await
        .db()?;
    Ok(())
}

async fn game_id_in_tx(
    conn: &mut SqliteConnection,
    source: Source,
    external_id: &str,
) -> Result<Option<i64>> {
    sqlx::query_scalar::<_, i64>("SELECT id FROM games WHERE source = ? AND external_id = ?")
        .bind(source.as_str())
        .bind(external_id)
        .fetch_optional(&mut *conn)
        .await
        .db()
}

async fn insert_game_in_tx(
    conn: &mut SqliteConnection,
    game: &Game,
    metadata_json: Option<String>,
    now: DateTime<Utc>,
) -> Result<i64> {
    let result = sqlx::query(
        "INSERT INTO games (
            source, external_id, media_kind, title, title_zh, short_description, cover_url, poster_url,
            release_date, release_raw, review_score, review_desc, review_count, is_free,
            price_currency, price_initial, price_final, discount_percent,
            developers, publishers, genres, platforms, tags, metadata_json, metadata_updated_at,
            created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(game.source.as_str())
    .bind(&game.external_id)
    .bind(game.media_kind.as_str())
    .bind(&game.title)
    .bind(game.title_zh.clone())
    .bind(game.short_description.clone())
    .bind(game.cover_url.clone())
    .bind(game.poster_url.clone())
    .bind(game.release_date.clone())
    .bind(game.release_raw.clone())
    .bind(game.review_score)
    .bind(game.review_desc.clone())
    .bind(game.review_count)
    .bind(game.is_free)
    .bind(game.price_currency.clone())
    .bind(game.price_initial)
    .bind(game.price_final)
    .bind(game.discount_percent)
    .bind(strings_json(&game.developers))
    .bind(strings_json(&game.publishers))
    .bind(strings_json(&game.genres))
    .bind(strings_json(&game.platforms))
    .bind(strings_json(&game.tags))
    .bind(metadata_json)
    .bind(game.metadata_updated_at)
    .bind(now)
    .bind(now)
    .execute(&mut *conn)
    .await
    .db()?;

    Ok(result.last_insert_rowid())
}

async fn update_game_in_tx(
    conn: &mut SqliteConnection,
    game: &Game,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "UPDATE games SET
            media_kind = ?, title = ?, title_zh = ?, short_description = ?, cover_url = ?, poster_url = ?,
            release_date = ?, release_raw = ?, review_score = ?, review_desc = ?, review_count = ?,
            is_free = ?, price_currency = ?, price_initial = ?, price_final = ?, discount_percent = ?,
            developers = ?, publishers = ?, genres = ?, platforms = ?, tags = ?,
            metadata_updated_at = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(game.media_kind.as_str())
    .bind(&game.title)
    .bind(game.title_zh.clone())
    .bind(game.short_description.clone())
    .bind(game.cover_url.clone())
    .bind(game.poster_url.clone())
    .bind(game.release_date.clone())
    .bind(game.release_raw.clone())
    .bind(game.review_score)
    .bind(game.review_desc.clone())
    .bind(game.review_count)
    .bind(game.is_free)
    .bind(game.price_currency.clone())
    .bind(game.price_initial)
    .bind(game.price_final)
    .bind(game.discount_percent)
    .bind(strings_json(&game.developers))
    .bind(strings_json(&game.publishers))
    .bind(strings_json(&game.genres))
    .bind(strings_json(&game.platforms))
    .bind(strings_json(&game.tags))
    .bind(game.metadata_updated_at)
    .bind(now)
    .bind(game.id)
    .execute(&mut *conn)
    .await
    .db()?;
    Ok(())
}

async fn insert_entry_in_tx(conn: &mut SqliteConnection, entry: &LibraryEntry) -> Result<()> {
    sqlx::query(
        "INSERT INTO library_entries (
            game_id, status, priority, favorite, rating, progress_percent, playtime_minutes,
            source_playtime_minutes, achievements_achieved, achievements_total,
            achievements_checked_at, note, source_added_at, started_at, finished_at,
            last_synced_at, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(entry.game_id)
    .bind(entry.status.as_str())
    .bind(entry.priority)
    .bind(entry.favorite)
    .bind(entry.rating)
    .bind(entry.progress_percent)
    .bind(entry.playtime_minutes)
    .bind(entry.source_playtime_minutes)
    .bind(entry.achievements_achieved)
    .bind(entry.achievements_total)
    .bind(entry.achievements_checked_at)
    .bind(entry.note.clone())
    .bind(entry.source_added_at)
    .bind(entry.started_at)
    .bind(entry.finished_at)
    .bind(entry.last_synced_at)
    .bind(entry.created_at)
    .bind(entry.updated_at)
    .execute(&mut *conn)
    .await
    .db()?;
    Ok(())
}

async fn update_entry_in_tx(conn: &mut SqliteConnection, entry: &LibraryEntry) -> Result<()> {
    sqlx::query(
        "UPDATE library_entries SET
            status = ?, priority = ?, favorite = ?, rating = ?, progress_percent = ?,
            playtime_minutes = ?, source_playtime_minutes = ?,
            achievements_achieved = ?, achievements_total = ?, achievements_checked_at = ?,
            note = ?, source_added_at = ?, started_at = ?, finished_at = ?,
            last_synced_at = ?, updated_at = ?
         WHERE game_id = ?",
    )
    .bind(entry.status.as_str())
    .bind(entry.priority)
    .bind(entry.favorite)
    .bind(entry.rating)
    .bind(entry.progress_percent)
    .bind(entry.playtime_minutes)
    .bind(entry.source_playtime_minutes)
    .bind(entry.achievements_achieved)
    .bind(entry.achievements_total)
    .bind(entry.achievements_checked_at)
    .bind(entry.note.clone())
    .bind(entry.source_added_at)
    .bind(entry.started_at)
    .bind(entry.finished_at)
    .bind(entry.last_synced_at)
    .bind(entry.updated_at)
    .bind(entry.game_id)
    .execute(&mut *conn)
    .await
    .db()?;
    Ok(())
}

async fn ensure_entry_in_tx(
    conn: &mut SqliteConnection,
    game_id: i64,
    status: &Status,
    source_added_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO library_entries (game_id, status, favorite, source_added_at, created_at, updated_at, last_synced_at)
         VALUES (?, ?, 0, ?, ?, ?, ?)
         ON CONFLICT (game_id) DO NOTHING",
    )
    .bind(game_id)
    .bind(status.as_str())
    .bind(source_added_at)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&mut *conn)
    .await
    .db()?;
    Ok(())
}
