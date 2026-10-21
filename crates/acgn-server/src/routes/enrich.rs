//! 批量补全元数据（appdetails），并按账号区域补价格。
//!
//! 两段式：
//! 1. **全局元数据**：用配置区域（`STEAM_COUNTRY`）抓详情，写入 `games`；
//! 2. **账号区域价格**：只对「区域与全局配置不同、且还没问过 Steam」的账号关联，
//!    按国家分组各抓一次，写入 `game_accounts.price_*`。与全局区域相同的账号不重复
//!    抓价，展示层直接回退到全局价格（同一区域的价格是一样的）。
//!
//! 两段共用同一套并发 / 节流配置；任一段触发 429 都会优雅停止，已写入的结果保留。

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use acgn_core::{EnrichSummary, Error, Source, SourceItem};
use acgn_steam::AppDetailsBatch;
use acgn_store::{AccountPrice, AccountPriceTarget, MetadataOutcome, RunCounts};
use axum::Json;
use axum::extract::State;
use chrono::Utc;
use serde::Deserialize;

use crate::error::{ApiError, ApiResult};
use crate::extract::ValidJson;
use crate::state::AppState;

/// POST /api/enrich
#[derive(Debug, Deserialize)]
pub struct EnrichRequest {
    /// 一次最多补全多少条（默认 20，限流保护）
    #[serde(default)]
    pub limit: Option<i64>,
}

pub async fn enrich(
    State(state): State<AppState>,
    ValidJson(request): ValidJson<EnrichRequest>,
) -> ApiResult<Json<EnrichSummary>> {
    let started = Instant::now();
    let limit = request.limit.unwrap_or(20).clamp(1, 200);

    // 目前只有 Steam 需要补全；未来 Bangumi 详情走各自的适配器。
    let pending = state
        .db
        .pending_metadata(Some(Source::Steam), limit)
        .await?;
    let appids: Vec<String> = pending
        .iter()
        .map(|item| item.external_id.clone())
        .collect();

    if appids.is_empty() {
        // 元数据都补全了，但账号区域价格可能还欠着：继续走第二段
        let pass = fill_account_prices(&state, limit).await?;
        if pass.attempted == 0 && pass.warnings.is_empty() {
            return Ok(Json(EnrichSummary {
                requested: 0,
                succeeded: 0,
                failed: 0,
                skipped: 0,
                account_prices: 0,
                warnings: vec!["没有需要补全的条目".to_string()],
                source_mode: state.steam.mode().to_string(),
                duration_ms: started.elapsed().as_millis() as i64,
            }));
        }

        let duration_ms = started.elapsed().as_millis() as i64;

        // 只有价格补全也算一次运行：否则前端「同步历史」里会缺失这段动作
        if pass.attempted > 0 {
            let run_id = state
                .db
                .begin_run(
                    Source::Steam,
                    "metadata_enrich",
                    Some(serde_json::json!({
                        "limit": limit,
                        "phase": "account_prices",
                        "account_prices": pass.written,
                    })),
                )
                .await?;
            state
                .db
                .finish_run_ok(
                    run_id,
                    RunCounts {
                        seen: pass.attempted,
                        added: 0,
                        updated: pass.written,
                        unchanged: 0,
                        skipped: 0,
                        // 与元数据补全一致：没写成功的条目按「抓取失败」记，
                        // 不要把失败混进 unchanged（unchanged = 抓到了但没有差异）。
                        missing: pass.attempted - pass.written,
                    },
                    &pass.warnings,
                    duration_ms,
                )
                .await?;
        }

        return Ok(Json(EnrichSummary {
            requested: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            account_prices: pass.written,
            warnings: pass.warnings,
            source_mode: state.steam.mode().to_string(),
            duration_ms,
        }));
    }

    let AppDetailsBatch {
        items,
        mut warnings,
        last_error,
    } = state.steam.app_details_batch(&appids).await?;

    // 全都被限流/拒绝时直接返回 422：让用户看到「稍后重试」，而不是伪造一个 0 成功的 200
    if items.is_empty()
        && let Some(err) = last_error
    {
        return Err(ApiError(err));
    }

    let mut succeeded = 0i64;
    let mut updated = 0i64;
    for item in &items {
        match state.db.update_metadata(item).await? {
            MetadataOutcome::Added(_) | MetadataOutcome::Updated(_) => {
                succeeded += 1;
                updated += 1;
            }
            MetadataOutcome::Unchanged(_) => succeeded += 1,
        }
    }

    let failed = appids.len() as i64 - items.len() as i64;
    if failed > 0 {
        warnings.push(format!(
            "{failed} 个条目没取到详情（限流或已下架），可以稍后再次点「补全」"
        ));
    }

    // 元数据被限流时别再接着打价格接口：一样会被拒绝
    let rate_limited = matches!(last_error, Some(Error::SourceRejected(_)));
    let account_prices = if rate_limited {
        warnings.push("已触发 Steam 限流：账号区域价格下次补全时继续".to_string());
        0
    } else {
        let pass = fill_account_prices(&state, limit).await?;
        warnings.extend(pass.warnings);
        pass.written
    };

    let duration_ms = started.elapsed().as_millis() as i64;

    // 记录一次运行，方便在前端看到「什么时候补过元数据」
    let run_id = state
        .db
        .begin_run(
            Source::Steam,
            "metadata_enrich",
            Some(serde_json::json!({
                "limit": limit,
                "account_prices": account_prices,
            })),
        )
        .await?;
    state
        .db
        .finish_run_ok(
            run_id,
            RunCounts {
                seen: appids.len() as i64,
                added: 0,
                updated,
                unchanged: succeeded - updated,
                skipped: 0,
                missing: failed,
            },
            &warnings,
            duration_ms,
        )
        .await?;

    Ok(Json(EnrichSummary {
        requested: appids.len() as i64,
        succeeded,
        failed,
        skipped: 0,
        account_prices,
        warnings,
        source_mode: state.steam.mode().to_string(),
        duration_ms,
    }))
}

/// 第二段的结果。
struct PricePass {
    /// 本轮排队处理的关联行数（同一国家 / 游戏会合并请求，但行数按关联算）
    attempted: i64,
    /// 成功写入价格的关联行数
    written: i64,
    warnings: Vec<String>,
}

/// 第二段：按账号区域补价格。
///
/// 同一国家下的 appid 去重后一次批量抓取；抓不到（限流 / 下架）的关联保持
/// `price_updated_at IS NULL`，下次补全再试。
async fn fill_account_prices(state: &AppState, limit: i64) -> ApiResult<PricePass> {
    let targets = state
        .db
        .pending_account_prices(Source::Steam, &state.config.steam.country, limit)
        .await?;
    if targets.is_empty() {
        return Ok(PricePass {
            attempted: 0,
            written: 0,
            warnings: Vec::new(),
        });
    }

    let attempted = targets.len() as i64;
    let mut grouped: BTreeMap<String, Vec<AccountPriceTarget>> = BTreeMap::new();
    for target in targets {
        grouped
            .entry(target.country.clone())
            .or_default()
            .push(target);
    }

    let mut written = 0i64;
    let mut warnings = Vec::new();

    for (country, targets) in grouped {
        let mut appids: Vec<String> = targets.iter().map(|t| t.external_id.clone()).collect();
        appids.sort();
        appids.dedup();

        let batch = match state
            .steam
            .app_details_batch_for_country(&appids, &country)
            .await
        {
            Ok(batch) => batch,
            Err(err) => {
                warnings.push(format!("账号区域 {country} 的价格抓取失败：{err}"));
                break;
            }
        };

        let AppDetailsBatch {
            items,
            warnings: batch_warnings,
            last_error,
        } = batch;
        let mut by_appid: HashMap<String, SourceItem> = items
            .into_iter()
            .map(|item| (item.external_id.clone(), item))
            .collect();

        let mut missing = Vec::new();
        for target in targets {
            let Some(item) = by_appid.remove(&target.external_id) else {
                missing.push(target);
                continue;
            };
            let price = AccountPrice {
                currency: item.price_currency.clone(),
                initial: item.price_initial,
                final_price: item.price_final,
                discount_percent: item.discount_percent,
                is_free: item.is_free,
            };
            if state
                .db
                .update_account_price(target.game_id, target.account_id, &price, Utc::now())
                .await?
            {
                written += 1;
            }
        }

        // 一批请求全部正常回答、只是某些 appid 在該区域没有可售条目（success:false /
        // 已下架）时，把关联标记为「已查过、无价格」，否则每次补全都会重复请求同一个
        // appid，永远收敛不了。只要本轮有请求级错误（网络 / 限流），就保持 pending 重试。
        if last_error.is_none() {
            for target in missing {
                if state
                    .db
                    .update_account_price(
                        target.game_id,
                        target.account_id,
                        &AccountPrice::default(),
                        Utc::now(),
                    )
                    .await?
                {
                    written += 1;
                }
            }
        }

        for warning in batch_warnings {
            warnings.push(format!("账号区域 {country}：{warning}"));
        }
        if matches!(last_error, Some(Error::SourceRejected(_))) {
            warnings.push(format!(
                "账号区域 {country} 的价格触发 Steam 限流，剩余条目下次补全时继续"
            ));
            break;
        }
    }

    Ok(PricePass {
        attempted,
        written,
        warnings,
    })
}
