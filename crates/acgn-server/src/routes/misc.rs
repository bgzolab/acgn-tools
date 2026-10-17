//! 杂项：前端配置、全量导出。

use axum::Json;
use axum::extract::State;
use chrono::Utc;
use serde_json::{Value, json};

use crate::config::PublicConfig;
use crate::error::ApiResult;
use crate::state::AppState;

/// GET /api/config
pub async fn config(State(state): State<AppState>) -> Json<PublicConfig> {
    Json(state.config.public())
}

/// GET /api/export/json —— 全量导出，用于备份和未来做多端同步。
pub async fn export_json(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let items = state.db.export_all().await?;

    Ok(Json(json!({
        "exported_at": Utc::now(),
        "app": acgn_core::APP_NAME,
        "version": acgn_core::VERSION,
        "schema_revision": acgn_core::SCHEMA_REVISION,
        "count": items.len(),
        "items": items,
    })))
}
