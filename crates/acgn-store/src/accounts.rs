//! 数据源账号：建号、资料更新、游戏关联、按区域价格。
//!
//! 账号是**数据源身份**（SteamID64），不是本项目的多用户体系；创建/更新都是幂等的：
//! 同一份资料重复写入不会刷新 `updated_at`（与 `docs/design.md` §4.4 一致）。
//!
//! `game_accounts` 保存「游戏 ↔ 账号」的多对多关联：库存（含时长）、愿望单标记，
//! 以及按该账号区域抓到的价格。条目列表按需批量回填（[`Db::attach_accounts`]），
//! 避免每条游戏都查一次。

use std::collections::HashMap;

use acgn_core::{
    Account, AccountProfile, AccountSummary, Error, GameAccount, GameWithEntry, Result, Source,
};
use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, SqliteConnection};

use crate::Db;
use crate::error::ResultExt;
use crate::rows::{
    AccountLinkRow, AccountRow, AccountSummaryRow, GameAccountRow, SELECT_GAME_ACCOUNT,
};

/// 按账号区域补价的候选（一条 = 一个游戏 × 一个账号）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPriceTarget {
    pub game_id: i64,
    pub account_id: i64,
    pub external_id: String,
    pub country: String,
}

/// 某个账号区域的价格（单位：分）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountPrice {
    pub currency: Option<String>,
    pub initial: Option<i64>,
    /// `final` 是 Rust 保留字，用 `final_price` 表示折后价
    pub final_price: Option<i64>,
    pub discount_percent: Option<i64>,
    pub is_free: Option<bool>,
}

impl Db {
    // ---------------------------------------------------------------- 账号

    /// 按 `(source, external_id)` 建号或更新资料。
    ///
    /// 返回 `(账号, 是否本次新建)`。资料字段为 `None` 时保留已有值；`metadata_updated_at`
    /// 只在真的抓到资料时推进，抓取失败（`profile = None`）只保证账号存在。
    pub async fn upsert_account(
        &self,
        source: Source,
        external_id: &str,
        profile: Option<&AccountProfile>,
    ) -> Result<(Account, bool)> {
        let external_id = external_id.trim();
        if external_id.is_empty() {
            return Err(Error::invalid("账号 ID 不能为空"));
        }

        let now = Utc::now();
        let mut tx = self.pool.begin().await.db()?;

        // 先查再插：`INSERT ... ON CONFLICT DO NOTHING` 即使冲突也会消耗
        // AUTOINCREMENT 序号，长期同步会让账号 id 出现大量空洞。
        let existing: Option<AccountRow> =
            sqlx::query_as("SELECT * FROM accounts WHERE source = ? AND external_id = ?")
                .bind(source.as_str())
                .bind(external_id)
                .fetch_optional(&mut *tx)
                .await
                .db()?;

        let created = if existing.is_some() {
            false
        } else {
            let inserted = sqlx::query(
                "INSERT INTO accounts (
                    source, external_id, display_name, avatar_url, profile_url, country_code,
                    metadata_json, metadata_updated_at, created_at, updated_at
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT (source, external_id) DO NOTHING",
            )
            .bind(source.as_str())
            .bind(external_id)
            .bind(
                profile
                    .and_then(|p| p.display_name.as_deref())
                    .map(str::trim),
            )
            .bind(profile.and_then(|p| p.avatar_url.as_deref()))
            .bind(profile.and_then(|p| p.profile_url.as_deref()))
            .bind(profile.and_then(|p| normalize_country(p.country_code.as_deref())))
            .bind(
                profile
                    .and_then(|p| p.metadata.as_ref())
                    .map(|v| v.to_string()),
            )
            .bind(profile.map(|_| now))
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .db()?;
            inserted.rows_affected() > 0
        };

        let current: AccountRow =
            sqlx::query_as("SELECT * FROM accounts WHERE source = ? AND external_id = ?")
                .bind(source.as_str())
                .bind(external_id)
                .fetch_one(&mut *tx)
                .await
                .db()?;

        if let Some(profile) = profile {
            let next = merge_profile(&current, profile);
            // 内容真的变化才刷新 updated_at；只是「又抓了一遍资料」不算内容变化
            // （metadata_updated_at 是抓取时间，每次成功抓取都会推进）。
            if next != current {
                let country_before = current
                    .country_override
                    .clone()
                    .or_else(|| current.country_code.clone());
                let country_after = next
                    .country_override
                    .clone()
                    .or_else(|| next.country_code.clone());

                let mut next = next;
                next.metadata_updated_at = Some(now);
                next.updated_at = now;
                sqlx::query(
                    "UPDATE accounts SET display_name = ?, avatar_url = ?, profile_url = ?,
                        country_code = ?, metadata_json = ?, metadata_updated_at = ?, updated_at = ?
                     WHERE id = ?",
                )
                .bind(&next.display_name)
                .bind(&next.avatar_url)
                .bind(&next.profile_url)
                .bind(&next.country_code)
                .bind(&next.metadata_json)
                .bind(next.metadata_updated_at)
                .bind(next.updated_at)
                .bind(current.id)
                .execute(&mut *tx)
                .await
                .db()?;

                // 资料区域变了（例如 loccountrycode 从空变为可用）：旧区域的价格
                // 全部作废，等下次「补全」按新区域重抓，避免一直展示旧币种。
                if country_before != country_after {
                    sqlx::query(
                        "UPDATE game_accounts SET price_currency = NULL, price_initial = NULL,
                            price_final = NULL, discount_percent = NULL, is_free = NULL,
                            price_updated_at = NULL, updated_at = ?
                         WHERE account_id = ?",
                    )
                    .bind(now)
                    .bind(current.id)
                    .execute(&mut *tx)
                    .await
                    .db()?;
                }

                tx.commit().await.db()?;
                return Ok((next.into_domain()?, created));
            }

            sqlx::query("UPDATE accounts SET metadata_updated_at = ? WHERE id = ?")
                .bind(now)
                .bind(current.id)
                .execute(&mut *tx)
                .await
                .db()?;
            tx.commit().await.db()?;
            let mut refreshed = current;
            refreshed.metadata_updated_at = Some(now);
            return Ok((refreshed.into_domain()?, created));
        }

        tx.commit().await.db()?;
        Ok((current.into_domain()?, created))
    }

    pub async fn get_account(&self, id: i64) -> Result<Option<Account>> {
        let row: Option<AccountRow> = sqlx::query_as("SELECT * FROM accounts WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .db()?;
        row.map(AccountRow::into_domain).transpose()
    }

    pub async fn find_account(&self, source: Source, external_id: &str) -> Result<Option<Account>> {
        let row: Option<AccountRow> =
            sqlx::query_as("SELECT * FROM accounts WHERE source = ? AND external_id = ?")
                .bind(source.as_str())
                .bind(external_id)
                .fetch_optional(&self.pool)
                .await
                .db()?;
        row.map(AccountRow::into_domain).transpose()
    }

    /// 全部账号 + 关联统计（库存 / 愿望单条目数）。
    pub async fn list_accounts(&self) -> Result<Vec<AccountSummary>> {
        let rows: Vec<AccountSummaryRow> = sqlx::query_as(
            "SELECT a.*,
                (SELECT COUNT(*) FROM game_accounts ga
                  WHERE ga.account_id = a.id AND ga.source_playtime_minutes IS NOT NULL) AS owned_count,
                (SELECT COUNT(*) FROM game_accounts ga
                  WHERE ga.account_id = a.id AND ga.in_wishlist = 1) AS wishlist_count
             FROM accounts a
             ORDER BY a.source ASC, a.id ASC",
        )
        .fetch_all(&self.pool)
        .await
        .db()?;

        rows.into_iter()
            .map(AccountSummaryRow::into_domain)
            .collect()
    }

    /// 设置 / 清除手动价格区域。
    ///
    /// 区域变化后清掉该账号已缓存的价格：旧区域的币种继续展示会误导用户，
    /// 下次「补全元数据」会按新区域重新抓。
    pub async fn set_account_country_override(
        &self,
        id: i64,
        country: Option<&str>,
    ) -> Result<Account> {
        let current = self
            .get_account(id)
            .await?
            .ok_or_else(|| Error::not_found(format!("账号 #{id}")))?;
        let next = normalize_country(country);
        if current.country_override == next {
            return Ok(current);
        }

        let now = Utc::now();
        let mut tx = self.pool.begin().await.db()?;
        sqlx::query("UPDATE accounts SET country_override = ?, updated_at = ? WHERE id = ?")
            .bind(&next)
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await
            .db()?;

        // 只有「实际取价区域」变了才需要清缓存（覆盖值与资料值相同时不必重抓）
        if current.country() != next.as_deref() {
            sqlx::query(
                "UPDATE game_accounts SET price_currency = NULL, price_initial = NULL,
                    price_final = NULL, discount_percent = NULL, is_free = NULL,
                    price_updated_at = NULL, updated_at = ?
                 WHERE account_id = ?",
            )
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await
            .db()?;
        }
        tx.commit().await.db()?;

        self.get_account(id)
            .await?
            .ok_or_else(|| Error::not_found(format!("账号 #{id}")))
    }

    /// 删除账号：关联（`game_accounts`）随外键级联删除，**游戏条目不删**。
    pub async fn delete_account(&self, id: i64) -> Result<bool> {
        let result = sqlx::query("DELETE FROM accounts WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .db()?;
        Ok(result.rows_affected() > 0)
    }

    /// 记账：最近一次同步时间（导入 / 成就）。
    pub async fn touch_account_synced(&self, id: i64, now: DateTime<Utc>) -> Result<()> {
        sqlx::query("UPDATE accounts SET last_synced_at = ? WHERE id = ?")
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .db()?;
        Ok(())
    }

    // ---------------------------------------------------------------- 游戏关联

    /// 某个游戏的全部账号关联（按账号 id 排序）。
    pub async fn accounts_for_game(&self, game_id: i64) -> Result<Vec<GameAccount>> {
        let mut qb = QueryBuilder::new(SELECT_GAME_ACCOUNT);
        qb.push(" WHERE ga.game_id = ").push_bind(game_id);
        qb.push(" ORDER BY ga.account_id ASC");

        let rows: Vec<GameAccountRow> = qb.build_query_as().fetch_all(&self.pool).await.db()?;
        rows.into_iter().map(GameAccountRow::into_domain).collect()
    }

    /// 批量回填 `GameWithEntry.accounts`（一次 IN 查询，避免 N+1）。
    pub async fn attach_accounts(&self, items: &mut [GameWithEntry]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        let mut qb = QueryBuilder::new(SELECT_GAME_ACCOUNT);
        qb.push(" WHERE ga.game_id IN (");
        let mut separated = qb.separated(", ");
        for item in items.iter() {
            separated.push_bind(item.game.id);
        }
        separated.push_unseparated(") ORDER BY ga.account_id ASC");

        let rows: Vec<GameAccountRow> = qb.build_query_as().fetch_all(&self.pool).await.db()?;
        let mut by_game: HashMap<i64, Vec<GameAccount>> = HashMap::new();
        for row in rows {
            let game_id = row.game_id;
            by_game.entry(game_id).or_default().push(row.into_domain()?);
        }

        for item in items.iter_mut() {
            item.accounts = by_game.remove(&item.game.id).unwrap_or_default();
        }
        Ok(())
    }

    // ---------------------------------------------------------------- 区域价格

    /// 需要按账号区域补价的候选：`price_updated_at IS NULL`、区域已知且与全局配置不同。
    ///
    /// 与全局区域相同的账号不重复抓：展示层回退到 `games.price_*`（同一区域的价格）。
    pub async fn pending_account_prices(
        &self,
        source: Source,
        config_country: &str,
        limit: i64,
    ) -> Result<Vec<AccountPriceTarget>> {
        let rows: Vec<(i64, i64, String, String)> = sqlx::query_as(
            "SELECT ga.game_id, ga.account_id, g.external_id,
                    LOWER(COALESCE(a.country_override, a.country_code))
             FROM game_accounts ga
             JOIN games g ON g.id = ga.game_id
             JOIN accounts a ON a.id = ga.account_id
             WHERE g.source = ?
               AND ga.price_updated_at IS NULL
               AND COALESCE(a.country_override, a.country_code) IS NOT NULL
               AND LOWER(COALESCE(a.country_override, a.country_code)) <> LOWER(?)
             ORDER BY ga.game_id ASC, ga.account_id ASC
             LIMIT ?",
        )
        .bind(source.as_str())
        .bind(config_country)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await
        .db()?;

        Ok(rows
            .into_iter()
            .map(
                |(game_id, account_id, external_id, country)| AccountPriceTarget {
                    game_id,
                    account_id,
                    external_id,
                    country,
                },
            )
            .collect())
    }

    /// 写入某个账号区域的价格（`price_updated_at` 表示「这个区域问过 Steam 了」）。
    pub async fn update_account_price(
        &self,
        game_id: i64,
        account_id: i64,
        price: &AccountPrice,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE game_accounts SET
                price_currency = ?, price_initial = ?, price_final = ?, discount_percent = ?,
                is_free = ?, price_updated_at = ?, updated_at = ?
             WHERE game_id = ? AND account_id = ?",
        )
        .bind(&price.currency)
        .bind(price.initial)
        .bind(price.final_price)
        .bind(price.discount_percent)
        .bind(price.is_free)
        .bind(now)
        .bind(now)
        .bind(game_id)
        .bind(account_id)
        .execute(&self.pool)
        .await
        .db()?;
        Ok(result.rows_affected() > 0)
    }
}

/// 资料归一化：只覆盖「来源给了值」的字段；`metadata` 按 JSON 语义比较，避免每次
/// 同步都因为对象键顺序 / 空白差异改写内容。`metadata_updated_at` 由调用方维护。
fn merge_profile(current: &AccountRow, profile: &AccountProfile) -> AccountRow {
    let mut next = current.clone();

    if let Some(name) = profile.display_name.as_deref().map(str::trim)
        && !name.is_empty()
    {
        next.display_name = Some(name.to_string());
    }
    if let Some(avatar) = profile.avatar_url.as_deref().filter(|s| !s.is_empty()) {
        next.avatar_url = Some(avatar.to_string());
    }
    if let Some(url) = profile.profile_url.as_deref().filter(|s| !s.is_empty()) {
        next.profile_url = Some(url.to_string());
    }
    if let Some(country) = normalize_country(profile.country_code.as_deref()) {
        next.country_code = Some(country);
    }

    if let Some(metadata) = profile.metadata.as_ref() {
        let next_raw = metadata.to_string();
        let same = next
            .metadata_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .is_some_and(|existing| existing == *metadata);
        if !same {
            next.metadata_json = Some(next_raw);
        }
    }

    next
}

/// 区域代码归一化：小写、去空白；空串视为「没有」。
fn normalize_country(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

/// 读取某条关联（导入合并用；预览与真实导入共用同一份判定）。
pub(crate) async fn load_account_link(
    conn: &mut SqliteConnection,
    game_id: i64,
    account_id: i64,
) -> Result<Option<AccountLinkRow>> {
    sqlx::query_as(
        "SELECT in_wishlist, source_playtime_minutes, playtime_2weeks_minutes, last_played_at
         FROM game_accounts WHERE game_id = ? AND account_id = ?",
    )
    .bind(game_id)
    .bind(account_id)
    .fetch_optional(&mut *conn)
    .await
    .db()
}

/// 该游戏所有账号的累计时长聚合（`MAX`，忽略 NULL）。
///
/// 多账号拥有同一个游戏时，有效时长取「玩得最久的那个账号」：来源只测得到单个账号，
/// 本地展示不应因为最后同步的是小号就把主号时长覆盖成 0。
pub(crate) async fn aggregate_playtime(
    conn: &mut SqliteConnection,
    game_id: i64,
    excluding_account: Option<i64>,
) -> Result<Option<i64>> {
    let mut qb = QueryBuilder::new(
        "SELECT MAX(source_playtime_minutes) FROM game_accounts WHERE game_id = ",
    );
    qb.push_bind(game_id);
    if let Some(account_id) = excluding_account {
        qb.push(" AND account_id <> ").push_bind(account_id);
    }
    qb.build_query_scalar::<Option<i64>>()
        .fetch_one(&mut *conn)
        .await
        .db()
}
