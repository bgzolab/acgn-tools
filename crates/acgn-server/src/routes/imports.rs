//! 导入 / 同步接口。

use std::time::Instant;

use acgn_core::{
    AchievementSyncSummary, Error, GameWithEntry, ImportOptions, ImportRun, ImportSummary,
    RestoreItem, Source, SourceItem,
};
use acgn_store::{ImportRequest, MissingScope, RunCounts};
use axum::Json;
use axum::extract::State;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{ApiError, ApiResult};
use crate::extract::{ValidJson, ValidPath, ValidQuery};
use crate::state::AppState;

/// 所有导入接口的统一返回。
#[derive(Debug, Serialize)]
pub struct ImportResponse {
    pub summary: ImportSummary,
    /// 运行记录；`dry_run`（仅预览）时为 `null`
    pub run: Option<ImportRun>,
    /// 本次新增 / 更新的少量条目，前端用来高亮
    pub preview: Vec<GameWithEntry>,
}

/// POST /api/imports/steam/wishlist
#[derive(Debug, Deserialize)]
pub struct SteamWishlistRequest {
    /// 17 位 steamID64、个性域名或个人主页链接
    pub steam_id: String,
    #[serde(default)]
    pub options: ImportOptions,
    /// 是否返回预览条目（默认 true）
    #[serde(default = "default_true")]
    pub preview: bool,
}

fn default_true() -> bool {
    true
}

pub async fn import_steam_wishlist(
    State(state): State<AppState>,
    ValidJson(request): ValidJson<SteamWishlistRequest>,
) -> ApiResult<Json<ImportResponse>> {
    let fetch = state.steam.fetch_wishlist(&request.steam_id).await?;

    let summary = state
        .db
        .import_items(&ImportRequest {
            source: Source::Steam,
            mode: "steam_wishlist",
            items: &fetch.items,
            options: request.options,
            args: Some(json!({
                "input": request.steam_id,
                "steam_id": fetch.steam_id,
                "source_mode": fetch.source_mode,
                "resolved_via": fetch.resolved_via,
            })),
            source_mode: fetch.source_mode,
            extra_warnings: fetch.warnings,
            missing_scope: MissingScope::Wishlist,
        })
        .await?;

    let run = state.db.get_import_run(summary.run_id).await?;

    let preview = if request.preview {
        state.db.load_many(&summary.sample_ids).await?
    } else {
        Vec::new()
    };

    Ok(Json(ImportResponse {
        summary,
        run,
        preview,
    }))
}

/// POST /api/imports/steam/library —— 导入已拥有游戏（含游玩时长）。
///
/// 与愿望单不同：官方 `GetOwnedGames` 接口不接受匿名调用，需要 `STEAM_API_KEY`。
#[derive(Debug, Deserialize)]
pub struct SteamLibraryRequest {
    /// 17 位 steamID64、个性域名或个人主页链接
    pub steam_id: String,
    #[serde(default)]
    pub options: ImportOptions,
    /// 是否返回预览条目（默认 true）
    #[serde(default = "default_true")]
    pub preview: bool,
}

pub async fn import_steam_library(
    State(state): State<AppState>,
    ValidJson(request): ValidJson<SteamLibraryRequest>,
) -> ApiResult<Json<ImportResponse>> {
    let fetch = state.steam.fetch_library(&request.steam_id).await?;

    let summary = state
        .db
        .import_items(&ImportRequest {
            source: Source::Steam,
            mode: "steam_library",
            items: &fetch.items,
            options: request.options,
            args: Some(json!({
                "input": request.steam_id,
                "steam_id": fetch.steam_id,
                "source_mode": fetch.source_mode,
                "resolved_via": fetch.resolved_via,
            })),
            source_mode: fetch.source_mode,
            extra_warnings: fetch.warnings,
            // 库存导入只对「上次被库存同步见过」的条目负责
            missing_scope: MissingScope::Library,
        })
        .await?;

    let run = state.db.get_import_run(summary.run_id).await?;

    let preview = if request.preview {
        state.db.load_many(&summary.sample_ids).await?
    } else {
        Vec::new()
    };

    Ok(Json(ImportResponse {
        summary,
        run,
        preview,
    }))
}

/// 成就检查的并发与节流：2 并发 + 250ms 间隔 ≈ 每秒 8 个游戏，
/// 一次性检查整库（几百个）只需十几秒到一分钟。`GetPlayerAchievements` 的限额比
/// appdetails 宽松，但偶发 401/拒绝（实测出现过一次），所以不追求更快。
const ACHIEVEMENTS_CONCURRENCY: usize = 2;
const ACHIEVEMENTS_DELAY_MS: u64 = 250;

/// POST /api/imports/steam/achievements —— 检查成就进度（全成就 → 已通关）。
///
/// 与库存导入一样需要 `steam_id`（成就是「某账号的进度」）；
/// 逐游戏查询，轮询顺序由 `achievements_checked_at` 决定（最久没查的优先）。
#[derive(Debug, Deserialize)]
pub struct SteamAchievementsRequest {
    /// 17 位 steamID64、个性域名或个人主页链接
    pub steam_id: String,
    /// 一次最多检查多少个游戏（默认 200，最大 500）
    #[serde(default)]
    pub limit: Option<i64>,
}

pub async fn sync_steam_achievements(
    State(state): State<AppState>,
    ValidJson(request): ValidJson<SteamAchievementsRequest>,
) -> ApiResult<Json<AchievementSyncSummary>> {
    let started = Instant::now();
    let limit = request.limit.unwrap_or(200).clamp(1, 500);

    let (steam_id, resolved_via, mut warnings) = state.steam.resolve_id(&request.steam_id).await?;
    let pending = state.db.pending_achievements(Source::Steam, limit).await?;

    if pending.is_empty() {
        return Ok(Json(AchievementSyncSummary {
            checked: 0,
            promoted: 0,
            unchanged: 0,
            unavailable: 0,
            failed: 0,
            pending: 0,
            source_mode: state.steam.mode().to_string(),
            duration_ms: started.elapsed().as_millis() as i64,
            warnings: vec!["没有需要检查成就的条目（先同步库存）".to_string()],
        }));
    }

    let now = Utc::now();
    let mut checked = 0i64;
    let mut promoted = 0i64;
    let mut unchanged = 0i64;
    let mut unavailable = 0i64;
    let mut failed = 0i64;
    let mut last_error: Option<Error> = None;
    let mut failure_samples: Vec<String> = Vec::new();

    'outer: for chunk in pending.chunks(ACHIEVEMENTS_CONCURRENCY) {
        let futures = chunk.iter().map(|entry| {
            let steam = state.steam.clone();
            let steam_id = steam_id.clone();
            async move {
                let result = steam
                    .fetch_player_achievements(&steam_id, &entry.external_id)
                    .await;
                (entry, result)
            }
        });
        let results = futures::future::join_all(futures).await;

        for (entry, result) in results {
            match result {
                Ok(progress) => {
                    let outcome = state
                        .db
                        .update_achievements(entry.game_id, progress, now)
                        .await?;
                    match (progress, outcome.promoted) {
                        (Some(_), true) => promoted += 1,
                        (Some(_), false) => unchanged += 1,
                        // 未拥有 / 隐私：取不到进度，只刷新了检查时间
                        (None, _) => unavailable += 1,
                    }
                    if progress.is_some() {
                        checked += 1;
                    }
                }
                Err(err) => {
                    failed += 1;
                    if failure_samples.len() < 5 {
                        failure_samples.push(format!("{}: {err}", entry.external_id));
                    }
                    let rejected = matches!(err, Error::SourceRejected(_));
                    last_error = Some(err);
                    if rejected {
                        warnings.push(
                            "已触发限流/拒绝，剩余条目稍后再点一次即可（已完成的会保留）"
                                .to_string(),
                        );
                        break 'outer;
                    }
                }
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(ACHIEVEMENTS_DELAY_MS)).await;
    }

    // 全部失败时直接返回错误（与 enrich 同款处理）：让用户看到「稍后重试」，
    // 而不是一个 0 成功的 200。（unavailable 是正常回答，不算失败。）
    if checked == 0
        && unavailable == 0
        && let Some(err) = last_error
    {
        return Err(ApiError(err));
    }

    let sample_count = failure_samples.len() as i64;
    warnings.extend(failure_samples);
    if failed > sample_count {
        warnings.push(format!(
            "共 {failed} 个条目请求失败（网络或接口异常），再点一次会重试"
        ));
    }
    if promoted > 0 {
        warnings.push(format!("{promoted} 个游戏全成就，已标记为「已通关」"));
    }

    let duration_ms = started.elapsed().as_millis() as i64;
    let pending_after = state.db.achievements_pending_count(Source::Steam).await?;

    let run_id = state
        .db
        .begin_run(
            Source::Steam,
            "steam_achievements",
            Some(json!({
                "input": request.steam_id,
                "steam_id": steam_id,
                "resolved_via": resolved_via,
                "limit": limit,
            })),
        )
        .await?;
    state
        .db
        .finish_run_ok(
            run_id,
            RunCounts {
                seen: checked + unavailable + failed,
                added: 0,
                updated: promoted,
                unchanged,
                skipped: unavailable + failed,
                missing: 0,
            },
            &warnings,
            duration_ms,
        )
        .await?;

    Ok(Json(AchievementSyncSummary {
        checked,
        promoted,
        unchanged,
        unavailable,
        failed,
        pending: pending_after,
        source_mode: state.steam.mode().to_string(),
        duration_ms,
        warnings,
    }))
}

/// POST /api/imports —— 通用 JSON 导入（离线导入 / 备份恢复 / 其它来源接入）
#[derive(Debug, Deserialize)]
pub struct JsonImportRequest {
    /// 请求级来源：给了就**覆盖**每条数据自己的 `source`
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub mode: Option<String>,
    pub items: Vec<SourceItem>,
    #[serde(default)]
    pub options: ImportOptions,
    #[serde(default = "default_true")]
    pub preview: bool,
}

pub async fn import_json(
    State(state): State<AppState>,
    ValidJson(request): ValidJson<JsonImportRequest>,
) -> ApiResult<Json<ImportResponse>> {
    if request.items.is_empty() {
        return Err(ApiError(Error::invalid("items 不能为空")));
    }

    let mode = request.mode.unwrap_or_else(|| "json".to_string());
    let mut items = request.items;

    // 混源导入会让 missing 统计与运行记录里的 source 只反映其中一个来源：
    // 要么显式用请求级 source 统一，要么拆成多次导入。
    if request.source.is_none() {
        let mut distinct: Vec<&'static str> =
            items.iter().map(|item| item.source.as_str()).collect();
        distinct.sort_unstable();
        distinct.dedup();
        if distinct.len() > 1 {
            return Err(ApiError(Error::invalid(format!(
                "items 里混用了多个来源（{}）：请显式指定请求级 source，或拆成多次导入",
                distinct.join(" / ")
            ))));
        }
    }

    // 请求级 source 覆盖条目自己的 source：离线导入时只需要写一次来源。
    let source = match request.source {
        Some(source) => {
            for item in items.iter_mut() {
                item.source = source;
            }
            source
        }
        None => items.first().map(|item| item.source).unwrap_or_default(),
    };

    let summary = state
        .db
        .import_items(&ImportRequest {
            source,
            mode: &mode,
            items: &items,
            options: request.options,
            args: Some(json!({ "items": items.len(), "source": source.as_str() })),
            source_mode: "json",
            extra_warnings: Vec::new(),
            missing_scope: MissingScope::Wishlist,
        })
        .await?;

    let run = state.db.get_import_run(summary.run_id).await?;

    let preview = if request.preview {
        state.db.load_many(&summary.sample_ids).await?
    } else {
        Vec::new()
    };

    Ok(Json(ImportResponse {
        summary,
        run,
        preview,
    }))
}

/// POST /api/imports/restore —— 备份还原（文件优先，不走来源合并）
///
/// 接受 `GET /api/export/json` 的整份导出文档（多余字段忽略）或裸数组 `[{game, entry}, …]`。
#[derive(Debug, Deserialize)]
pub struct RestoreRequest {
    pub items: Vec<RestoreItem>,
    /// 只统计不落库（与 Steam 导入的 `options.dry_run` 同义）
    #[serde(default)]
    pub dry_run: bool,
    /// 是否返回受影响的条目（默认 true）
    #[serde(default = "default_true")]
    pub preview: bool,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum RestoreBody {
    /// 导出文档，或手写的 `{ "items": [...] }`
    Document(RestoreRequest),
    /// 裸数组
    Items(Vec<RestoreItem>),
}

impl RestoreBody {
    fn into_parts(self) -> (Vec<RestoreItem>, bool, bool) {
        match self {
            RestoreBody::Document(request) => (request.items, request.dry_run, request.preview),
            RestoreBody::Items(items) => (items, false, true),
        }
    }
}

pub async fn restore_json(
    State(state): State<AppState>,
    ValidJson(body): ValidJson<RestoreBody>,
) -> ApiResult<Json<ImportResponse>> {
    let (items, dry_run, preview) = body.into_parts();

    let summary = state.db.restore_items(&items, dry_run).await?;
    let run = state.db.get_import_run(summary.run_id).await?;
    let preview_items = if preview && !dry_run {
        state.db.load_many(&summary.sample_ids).await?
    } else {
        Vec::new()
    };

    Ok(Json(ImportResponse {
        summary,
        run,
        preview: preview_items,
    }))
}

/// GET /api/imports?limit=20
#[derive(Debug, Deserialize)]
pub struct RunsQuery {
    pub limit: Option<i64>,
}

pub async fn list_runs(
    State(state): State<AppState>,
    ValidQuery(query): ValidQuery<RunsQuery>,
) -> ApiResult<Json<Vec<ImportRun>>> {
    let runs = state.db.list_import_runs(query.limit.unwrap_or(20)).await?;
    Ok(Json(runs))
}

/// GET /api/imports/{id}
pub async fn get_run(
    State(state): State<AppState>,
    ValidPath(id): ValidPath<i64>,
) -> ApiResult<Json<ImportRun>> {
    let run = state
        .db
        .get_import_run(id)
        .await?
        .ok_or_else(|| ApiError(Error::not_found(format!("运行记录 #{id}"))))?;
    Ok(Json(run))
}
