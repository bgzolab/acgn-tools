//! 健康检查。

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::error::ApiResult;
use crate::state::AppState;

pub async fn health(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    state.db.ping().await?;

    Ok(Json(json!({
        "status": "ok",
        "app": acgn_core::APP_NAME,
        "version": acgn_core::VERSION,
        "uptime_seconds": state.uptime_seconds(),
        "steam_mode": state.steam.mode(),
        "db": "ok",
    })))
}
