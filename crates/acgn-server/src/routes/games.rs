//! 游戏列表 / 详情 / 编辑 / 单条元数据刷新。

use acgn_core::{
    DeleteGamesRequest, DeleteGamesResponse, EntryPatch, Error, GameListQuery, GameWithEntry, Page,
    Source,
};
use acgn_store::GameFilters;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::extract::{ValidJson, ValidPath, ValidQuery};
use crate::state::AppState;

/// GET /api/games
pub async fn list_games(
    State(state): State<AppState>,
    ValidQuery(query): ValidQuery<GameListQuery>,
) -> ApiResult<Json<Page<GameWithEntry>>> {
    let filters = GameFilters::from_query(&query);
    let page = state
        .db
        .list_games(
            &filters,
            query.sort.as_deref(),
            query.page(),
            query.per_page(),
        )
        .await?;
    Ok(Json(page))
}

/// GET /api/games/{id}
pub async fn get_game(
    State(state): State<AppState>,
    ValidPath(id): ValidPath<i64>,
) -> ApiResult<Json<GameWithEntry>> {
    let item = state
        .db
        .get_game(id)
        .await?
        .ok_or_else(|| ApiError(Error::not_found(format!("游戏 #{id}"))))?;
    Ok(Json(item))
}

/// PATCH /api/games/{id}
pub async fn patch_game(
    State(state): State<AppState>,
    ValidPath(id): ValidPath<i64>,
    ValidJson(patch): ValidJson<EntryPatch>,
) -> ApiResult<Json<GameWithEntry>> {
    let updated = state.db.update_entry(id, &patch).await?;
    Ok(Json(updated))
}

/// DELETE /api/games/{id} —— 删除条目（本地动作；来源里还在的话，下次导入会重新加回来）。
pub async fn delete_game(
    State(state): State<AppState>,
    ValidPath(id): ValidPath<i64>,
) -> ApiResult<StatusCode> {
    if state.db.delete_game(id).await? {
        return Ok(StatusCode::NO_CONTENT);
    }
    Err(ApiError(Error::not_found(format!("游戏 #{id}"))))
}

/// POST /api/games/delete —— 批量删除（多选清理用），未知 id 静默忽略。
pub async fn delete_games(
    State(state): State<AppState>,
    ValidJson(request): ValidJson<DeleteGamesRequest>,
) -> ApiResult<Json<DeleteGamesResponse>> {
    if request.ids.is_empty() {
        return Err(ApiError(Error::invalid("ids 不能为空")));
    }
    if request.ids.len() > 1000 {
        return Err(ApiError(Error::invalid("一次最多删除 1000 条")));
    }

    let deleted = state.db.delete_games(&request.ids).await?;
    Ok(Json(DeleteGamesResponse { deleted }))
}

/// POST /api/games/{id}/refresh-metadata —— 抓取单个条目的商店详情。
pub async fn refresh_metadata(
    State(state): State<AppState>,
    ValidPath(id): ValidPath<i64>,
) -> ApiResult<Json<Value>> {
    let item = state
        .db
        .get_game(id)
        .await?
        .ok_or_else(|| ApiError(Error::not_found(format!("游戏 #{id}"))))?;

    if item.game.source != Source::Steam {
        return Err(ApiError(Error::invalid(format!(
            "条目 #{} 的来源是 {}，暂不支持刷新",
            id,
            item.game.source.label()
        ))));
    }

    let Some(source_item) = state.steam.app_details_one(&item.game.external_id).await? else {
        return Err(ApiError(Error::upstream(format!(
            "Steam 未返回 appid={} 的详情（可能已下架）",
            item.game.external_id
        ))));
    };

    let outcome = state.db.update_metadata(&source_item).await?;
    let refreshed = state
        .db
        .get_game(id)
        .await?
        .ok_or_else(|| ApiError(Error::not_found(format!("游戏 #{id}"))))?;

    Ok(Json(json!({
        "outcome": outcome.kind(),
        "game": refreshed.game,
        "entry": refreshed.entry,
    })))
}
