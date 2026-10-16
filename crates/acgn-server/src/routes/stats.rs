//! 看板统计。

use acgn_core::Stats;
use axum::Json;
use axum::extract::State;

use crate::error::ApiResult;
use crate::state::AppState;

/// GET /api/stats
pub async fn stats(State(state): State<AppState>) -> ApiResult<Json<Stats>> {
    let stats = state.db.stats().await?;
    Ok(Json(stats))
}
