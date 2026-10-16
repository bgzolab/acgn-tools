//! 批量补全元数据（appdetails）。

use std::time::Instant;

use acgn_core::{EnrichSummary, Source};
use acgn_steam::AppDetailsBatch;
use acgn_store::{MetadataOutcome, RunCounts};
use axum::Json;
use axum::extract::State;
use serde::Deserialize;

use crate::error::{ApiError, ApiResult};
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
    Json(request): Json<EnrichRequest>,
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
        return Ok(Json(EnrichSummary {
            requested: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            warnings: vec!["没有需要补全的条目".to_string()],
            source_mode: state.steam.mode().to_string(),
            duration_ms: started.elapsed().as_millis() as i64,
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

    let duration_ms = started.elapsed().as_millis() as i64;

    // 记录一次运行，方便在前端看到「什么时候补过元数据」
    let run_id = state
        .db
        .begin_run(
            Source::Steam,
            "metadata_enrich",
            Some(serde_json::json!({ "limit": limit })),
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
        warnings,
        source_mode: state.steam.mode().to_string(),
        duration_ms,
    }))
}
