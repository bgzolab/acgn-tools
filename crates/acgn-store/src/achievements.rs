//! 成就同步：检查 Steam 成就进度，并把「全成就」的条目标记为已通关。
//!
//! 数据来源是 `ISteamUserStats/GetPlayerAchievements`（每游戏一次请求），因此这是一个
//! **独立的按需操作**（像元数据补全一样），不塞进每次库存同步。轮询策略：
//! `achievements_checked_at` 为空的最先查，其余按最久没查的排前面——即使中途中断，
//! 再点一次也会从没查过的继续，而不是每次从头发起。

use acgn_core::{AchievementProgress, Error, Result, Source, Status};
use chrono::{DateTime, Utc};

use crate::Db;
use crate::error::ResultExt;

/// 待检查成就的条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAchievement {
    pub game_id: i64,
    pub external_id: String,
    pub title: String,
}

/// 单条成就更新的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AchievementOutcome {
    /// 是否因「全成就」把状态标记为已通关
    pub promoted: bool,
}

/// `library_entries` 里成就更新需要的列。
#[derive(Debug, sqlx::FromRow)]
struct AchievementRow {
    status: String,
    progress_percent: Option<i64>,
    finished_at: Option<DateTime<Utc>>,
    achievements_achieved: Option<i64>,
    achievements_total: Option<i64>,
}

impl Db {
    /// 取一批待检查成就的条目（`source_playtime_minutes` 非空 = 库存同步见过的条目）。
    ///
    /// 排序：从没查过的优先，其次最久没查的优先。
    pub async fn pending_achievements(
        &self,
        source: Source,
        limit: i64,
    ) -> Result<Vec<PendingAchievement>> {
        let rows: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT g.id, g.external_id, g.title
             FROM games g JOIN library_entries e ON e.game_id = g.id
             WHERE g.source = ? AND e.source_playtime_minutes IS NOT NULL
             ORDER BY e.achievements_checked_at IS NULL DESC, e.achievements_checked_at ASC, g.id ASC
             LIMIT ?",
        )
        .bind(source.as_str())
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await
        .db()?;

        Ok(rows
            .into_iter()
            .map(|(game_id, external_id, title)| PendingAchievement {
                game_id,
                external_id,
                title,
            })
            .collect())
    }

    /// 还没检查过成就的条目数（前端用来提示「还有 N 个，再点一次」）。
    pub async fn achievements_pending_count(&self, source: Source) -> Result<i64> {
        sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM games g JOIN library_entries e ON e.game_id = g.id
             WHERE g.source = ? AND e.source_playtime_minutes IS NOT NULL
               AND e.achievements_checked_at IS NULL",
        )
        .bind(source.as_str())
        .fetch_one(&self.pool)
        .await
        .db()
    }

    /// 写入一次成就检查的结果。
    ///
    /// * `progress = Some(_)`：写进度并刷新检查时间；`total > 0 && achieved >= total`
    ///   且本地状态还处于「愿望单 / 待玩 / 在玩」时，升级为已通关（用户手动标过的
    ///   已通关 / 搁置 / 弃坑 / 忽略永远不回退）；同时补 `finished_at` 与 100% 进度
    ///   （与手动切到已通关的语义一致，都只在为空时补）。
    /// * `progress = None`：取不到成就数据（未拥有 / 隐私 / 接口异常），只刷新检查时间，
    ///   保留上一次的进度值——下一轮轮询会再试。
    pub async fn update_achievements(
        &self,
        game_id: i64,
        progress: Option<AchievementProgress>,
        now: DateTime<Utc>,
    ) -> Result<AchievementOutcome> {
        let entry: Option<AchievementRow> = sqlx::query_as(
            "SELECT status, progress_percent, finished_at, achievements_achieved, achievements_total
             FROM library_entries WHERE game_id = ?",
        )
        .bind(game_id)
        .fetch_optional(&self.pool)
        .await
        .db()?;

        let Some(AchievementRow {
            status: status_raw,
            progress_percent,
            finished_at,
            achievements_achieved: previous_achieved,
            achievements_total: previous_total,
        }) = entry
        else {
            return Err(Error::not_found(format!("game #{game_id}")));
        };

        let status = Status::from_str_lossy(&status_raw)
            .ok_or_else(|| Error::storage(format!("非法 status: {status_raw}")))?;

        let mut promoted = false;
        let mut next_status = status;
        let mut next_progress = progress_percent;
        let mut next_finished = finished_at;

        if let Some(progress) = progress
            && progress.is_complete()
            && matches!(status, Status::Wishlist | Status::Backlog | Status::Playing)
        {
            next_status = Status::Completed;
            next_progress.get_or_insert(100);
            next_finished.get_or_insert(now);
            promoted = true;
        }

        // 取不到数据时保留上一次的进度：一次网络抖动不该把「46/46」清成未知
        let (achieved, total) = match progress {
            Some(progress) => (Some(progress.achieved), Some(progress.total)),
            None => (previous_achieved, previous_total),
        };

        sqlx::query(
            "UPDATE library_entries SET
                status = ?, progress_percent = ?, finished_at = ?,
                achievements_achieved = ?, achievements_total = ?, achievements_checked_at = ?,
                updated_at = ?
             WHERE game_id = ?",
        )
        .bind(next_status.as_str())
        .bind(next_progress)
        .bind(next_finished)
        .bind(achieved)
        .bind(total)
        .bind(now)
        .bind(now)
        .bind(game_id)
        .execute(&self.pool)
        .await
        .db()?;

        Ok(AchievementOutcome { promoted })
    }
}
