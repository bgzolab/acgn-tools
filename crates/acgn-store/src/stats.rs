//! 看板统计。

use acgn_core::{MoneyBucket, Result, Source, SourceCount, Stats, Status, StatusCount};
use std::collections::HashMap;

use crate::Db;
use crate::error::ResultExt;
use crate::games::GameFilters;

impl Db {
    /// 汇总看板数据；所有查询都很轻，直接实时算。
    pub async fn stats(&self) -> Result<Stats> {
        let total_games: i64 = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM games")
            .fetch_one(&self.pool)
            .await
            .db()?;

        let status_rows: Vec<(String, i64)> = sqlx::query_as::<_, (String, i64)>(
            "SELECT status, COUNT(*) FROM library_entries GROUP BY status",
        )
        .fetch_all(&self.pool)
        .await
        .db()?;
        let mut status_map: HashMap<String, i64> = status_rows.into_iter().collect();
        let by_status: Vec<StatusCount> = Status::ALL
            .iter()
            .map(|status| StatusCount {
                status: *status,
                label: status.label().to_string(),
                count: status_map.remove(status.as_str()).unwrap_or(0),
            })
            .collect();

        let source_rows: Vec<(String, i64)> = sqlx::query_as::<_, (String, i64)>(
            "SELECT source, COUNT(*) FROM games GROUP BY source",
        )
        .fetch_all(&self.pool)
        .await
        .db()?;
        let by_source: Vec<SourceCount> = source_rows
            .into_iter()
            .filter_map(|(source, count)| {
                Source::from_str_lossy(&source).map(|source| SourceCount {
                    source,
                    label: source.label().to_string(),
                    count,
                })
            })
            .collect();

        let favorite_count: i64 =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM library_entries WHERE favorite = 1")
                .fetch_one(&self.pool)
                .await
                .db()?;

        let total_playtime_minutes: i64 = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(SUM(playtime_minutes), 0) FROM library_entries",
        )
        .fetch_one(&self.pool)
        .await
        .db()?;

        let average_rating: Option<f64> = sqlx::query_scalar::<_, Option<f64>>(
            "SELECT AVG(CAST(rating AS REAL)) FROM library_entries WHERE rating IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await
        .db()?;

        let value_rows: Vec<(String, i64, i64, i64)> = sqlx::query_as::<_, (String, i64, i64, i64)>(
            "SELECT
                COALESCE(g.price_currency, 'UNKNOWN')                                          AS currency,
                COALESCE(SUM(COALESCE(g.price_initial, 0)), 0)                                 AS list_price_cents,
                COALESCE(SUM(COALESCE(g.price_final, g.price_initial, 0)), 0)                  AS current_price_cents,
                -- 只统计「有价格信息」的条目：否则还没补全元数据的愿望单条目会把条目数虚增，
                -- 与聚合金额的口径不一致（金额里它们贡献 0）
                COALESCE(SUM(CASE WHEN g.price_currency IS NOT NULL
                                    OR g.price_final IS NOT NULL
                                    OR g.price_initial IS NOT NULL
                             THEN 1 ELSE 0 END), 0)                                        AS item_count
             FROM games g
             JOIN library_entries e ON e.game_id = g.id
             WHERE e.status = 'wishlist' AND COALESCE(g.is_free, 0) = 0
             GROUP BY currency
             HAVING item_count > 0
             ORDER BY current_price_cents DESC",
        )
        .fetch_all(&self.pool)
        .await
        .db()?;
        let wishlist_value = value_rows
            .into_iter()
            .map(
                |(currency, list_price_cents, current_price_cents, item_count)| MoneyBucket {
                    currency,
                    list_price_cents,
                    current_price_cents,
                    item_count,
                },
            )
            .collect();

        let recent_imports = self.list_import_runs(6).await?;

        let recently_added = self
            .list_games(&GameFilters::default(), Some("added_desc"), 1, 8)
            .await?
            .items;

        let playing = self
            .list_games(
                &GameFilters {
                    status: Some(Status::Playing),
                    ..Default::default()
                },
                Some("updated_desc"),
                1,
                8,
            )
            .await?
            .items;

        Ok(Stats {
            total_games,
            by_status,
            by_source,
            favorite_count,
            total_playtime_minutes,
            average_rating,
            wishlist_value,
            recent_imports,
            recently_added,
            playing,
        })
    }
}
