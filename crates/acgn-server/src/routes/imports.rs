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
use tracing::warn;

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

/// 解析本次同步使用的账号。
///
/// 两种入口：
/// * `account_id`：从账号列表里选一个（推荐，前端「重同步」走这条）；
/// * `steam_id`：直接填 ID / 个性域名 / 主页链接，**自动建号**（兼容旧调用）。
///
/// 无论哪条路都会尽力刷新一次账号资料；资料抓取失败只记告警，不阻塞同步。
struct ResolvedAccount {
    /// 账号 id；`0` = 仅预览且账号尚不存在（不落库，按空关联预览）
    id: i64,
    external_id: String,
    label: String,
    warnings: Vec<String>,
}

/// 账号资料在这个时间窗内不重复抓取：一次「一键同步」有多个步骤，
/// 没必要每一步都请求 `GetPlayerSummaries`。手动「添加账号」始终会强制刷新。
const PROFILE_REFRESH_TTL_SECS: i64 = 300;

async fn resolve_request_account(
    state: &AppState,
    account_id: Option<i64>,
    steam_id: Option<&str>,
    refresh_profile: bool,
) -> ApiResult<ResolvedAccount> {
    if account_id.is_some() && steam_id.is_some_and(|value| !value.trim().is_empty()) {
        return Err(ApiError(Error::invalid(
            "account_id 与 steam_id 只能提供一个：选择账号就用 account_id，直接填 ID 才用 steam_id",
        )));
    }

    let mut warnings = Vec::new();
    let (input, existing, refresh) = match account_id {
        Some(account_id) => {
            let account = state
                .db
                .get_account(account_id)
                .await?
                .ok_or_else(|| ApiError(Error::not_found(format!("账号 #{account_id}"))))?;
            if account.source != Source::Steam {
                return Err(ApiError(Error::invalid(format!(
                    "账号 #{} 的来源是 {}，暂不支持同步",
                    account.id,
                    account.source.label()
                ))));
            }
            let stale = account
                .metadata_updated_at
                .is_none_or(|at| (Utc::now() - at).num_seconds() >= PROFILE_REFRESH_TTL_SECS);
            (
                account.external_id.clone(),
                Some(account),
                refresh_profile && stale,
            )
        }
        None => {
            let input = steam_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ApiError(Error::invalid(
                        "请提供 account_id（选择账号）或 steam_id（直接填写 ID / 个性域名）",
                    ))
                })?;
            (input.to_string(), None, refresh_profile)
        }
    };

    let resolved = match state.steam.resolve_id(&input).await {
        Ok((steam_id, _, mut extra)) => {
            warnings.append(&mut extra);
            steam_id
        }
        Err(err) if existing.is_some() => {
            // 已有账号的 ID 是解析好的 steamID64，理论上不会失败；失败也不阻塞同步
            warnings.push(format!("账号 ID 解析失败（{err}），沿用已保存的 ID"));
            existing
                .as_ref()
                .map(|account| account.external_id.clone())
                .unwrap_or(input)
        }
        Err(err) => return Err(ApiError(err)),
    };

    if refresh {
        let profile = match state.steam.fetch_profile(&resolved).await {
            Ok(profile) => Some(profile),
            Err(err) => {
                warnings.push(format!("账号资料刷新失败：{err}"));
                None
            }
        };
        let (account, _) = state
            .db
            .upsert_account(Source::Steam, &resolved, profile.as_ref())
            .await?;
        let label = account.display_label();
        return Ok(ResolvedAccount {
            id: account.id,
            external_id: account.external_id,
            label,
            warnings,
        });
    }

    // 不抓资料（dry-run / 资料仍在 TTL 内）：不写库。
    // 账号不存在（只可能来自 steam_id + 仅预览）时用 id 0 做空关联预览，
    // 这样「仅预览不写入」对 accounts 表也成立。
    let existing = match existing {
        Some(account) => Some(account),
        None => state.db.find_account(Source::Steam, &resolved).await?,
    };
    Ok(match existing {
        Some(account) => {
            let label = account.display_label();
            ResolvedAccount {
                id: account.id,
                external_id: account.external_id,
                label,
                warnings,
            }
        }
        None => ResolvedAccount {
            id: 0,
            external_id: resolved.clone(),
            label: format!("{} {resolved}", Source::Steam.label()),
            warnings,
        },
    })
}

/// POST /api/imports/steam/wishlist
#[derive(Debug, Deserialize)]
pub struct SteamWishlistRequest {
    /// 账号 id（从账号列表选择；推荐）
    #[serde(default)]
    pub account_id: Option<i64>,
    /// 兼容入口：17 位 steamID64、个性域名或个人主页链接（会自动建号）
    #[serde(default)]
    pub steam_id: Option<String>,
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
    let resolved_account = resolve_request_account(
        &state,
        request.account_id,
        request.steam_id.as_deref(),
        !request.options.dry_run,
    )
    .await?;
    let ResolvedAccount {
        id: account_id,
        external_id,
        label,
        mut warnings,
    } = resolved_account;

    let fetch = state.steam.fetch_wishlist(&external_id).await?;
    warnings.extend(fetch.warnings);

    let summary = state
        .db
        .import_items(&ImportRequest {
            source: Source::Steam,
            mode: "steam_wishlist",
            items: &fetch.items,
            options: request.options,
            args: Some(json!({
                "account_id": account_id,
                "account": label,
                "input": request.steam_id,
                "steam_id": fetch.steam_id,
                "source_mode": fetch.source_mode,
                "resolved_via": fetch.resolved_via,
            })),
            source_mode: fetch.source_mode,
            extra_warnings: warnings,
            missing_scope: MissingScope::Wishlist,
            account_id: Some(account_id),
        })
        .await?;

    let run = state.db.get_import_run(summary.run_id).await?;

    let preview = if request.preview {
        let mut preview = state.db.load_many(&summary.sample_ids).await?;
        state.db.attach_accounts(&mut preview).await?;
        preview
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
    /// 账号 id（从账号列表选择；推荐）
    #[serde(default)]
    pub account_id: Option<i64>,
    /// 兼容入口：17 位 steamID64、个性域名或个人主页链接（会自动建号）
    #[serde(default)]
    pub steam_id: Option<String>,
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
    let resolved_account = resolve_request_account(
        &state,
        request.account_id,
        request.steam_id.as_deref(),
        !request.options.dry_run,
    )
    .await?;
    let ResolvedAccount {
        id: account_id,
        external_id,
        label,
        mut warnings,
    } = resolved_account;

    let fetch = state.steam.fetch_library(&external_id).await?;
    warnings.extend(fetch.warnings);

    let summary = state
        .db
        .import_items(&ImportRequest {
            source: Source::Steam,
            mode: "steam_library",
            items: &fetch.items,
            options: request.options,
            args: Some(json!({
                "account_id": account_id,
                "account": label,
                "input": request.steam_id,
                "steam_id": fetch.steam_id,
                "source_mode": fetch.source_mode,
                "resolved_via": fetch.resolved_via,
            })),
            source_mode: fetch.source_mode,
            extra_warnings: warnings,
            // 库存导入只对「这个账号上次被库存同步见过」的条目负责
            missing_scope: MissingScope::Library,
            account_id: Some(account_id),
        })
        .await?;

    let run = state.db.get_import_run(summary.run_id).await?;

    let preview = if request.preview {
        let mut preview = state.db.load_many(&summary.sample_ids).await?;
        state.db.attach_accounts(&mut preview).await?;
        preview
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

/// 连续失败多少个请求就熔断本轮：系统性故障（Key 失效但返回 JSON、接口变更……）时
/// 不要空转打满 limit；正常回答（未拥有 / 没有统计）不会计入连续失败。
const MAX_CONSECUTIVE_ACHIEVEMENT_FAILURES: usize = 5;

/// POST /api/imports/steam/achievements —— 检查成就进度（全成就 → 已通关）。
///
/// 与库存导入一样需要「某个账号」：`account_id`（推荐）或 `steam_id`（自动建号）。
/// 逐游戏查询，轮询顺序由 `achievements_checked_at` 决定（最久没查的优先）。
#[derive(Debug, Deserialize)]
pub struct SteamAchievementsRequest {
    /// 账号 id（从账号列表选择；推荐）
    #[serde(default)]
    pub account_id: Option<i64>,
    /// 兼容入口：17 位 steamID64、个性域名或个人主页链接
    #[serde(default)]
    pub steam_id: Option<String>,
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

    let resolved = resolve_request_account(
        &state,
        request.account_id,
        request.steam_id.as_deref(),
        true,
    )
    .await?;
    let ResolvedAccount {
        id: account_id,
        external_id: steam_id,
        label,
        warnings: mut base_warnings,
    } = resolved;
    let pending = state
        .db
        .pending_achievements(Source::Steam, Some(account_id), limit)
        .await?;

    if pending.is_empty() {
        return Ok(Json(AchievementSyncSummary {
            checked: 0,
            updated: 0,
            promoted: 0,
            unchanged: 0,
            unavailable: 0,
            failed: 0,
            pending: 0,
            source_mode: state.steam.mode().to_string(),
            duration_ms: started.elapsed().as_millis() as i64,
            warnings: {
                base_warnings.push("没有需要检查成就的条目（先同步库存）".to_string());
                base_warnings
            },
        }));
    }

    // 先开运行记录：即使中途熔断/失败，同步历史里也有这一笔（AGENTS.md 约定）
    let run_id = state
        .db
        .begin_run(
            Source::Steam,
            "steam_achievements",
            Some(json!({
                "account_id": account_id,
                "account": label,
                "input": request.steam_id,
                "steam_id": steam_id,
                "limit": limit,
            })),
        )
        .await?;

    let mut checked = 0i64;
    let mut updated = 0i64;
    let mut promoted = 0i64;
    let mut unchanged = 0i64;
    let mut unavailable = 0i64;
    let mut failed = 0i64;
    let mut consecutive_failures = 0usize;
    let mut last_error: Option<Error> = None;
    let mut failure_samples: Vec<String> = Vec::new();
    let mut warnings = base_warnings;

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
                    // 每条取一次时间：整批可能跑一分钟，「检查于 …」应是实际检查时刻，
                    // 而不是批次开始时间（否则同轮内时钟全相同，排序退化为按 id）
                    match state
                        .db
                        .update_achievements(entry.game_id, progress, Utc::now())
                        .await
                    {
                        Ok(outcome) => {
                            // 只有「请求 + 写库」都成功才算一次完好处理
                            consecutive_failures = 0;
                            if progress.is_some() {
                                checked += 1;
                            } else {
                                // 未拥有 / 隐私：取不到进度，只刷新了检查时间
                                unavailable += 1;
                            }
                            if outcome.changed {
                                updated += 1;
                            } else if progress.is_some() {
                                unchanged += 1;
                            }
                            if outcome.promoted {
                                promoted += 1;
                            }
                        }
                        Err(err) => {
                            // 单条写库失败（典型：条目在轮询途中被删除）不该中止整批；
                            // 但连续的存储故障也要熔断，否则会打完整个 limit
                            failed += 1;
                            if failure_samples.len() < 5 {
                                failure_samples.push(format!("{}: {err}", entry.external_id));
                            }
                            if !matches!(err, Error::NotFound(_)) {
                                consecutive_failures += 1;
                                last_error = Some(err);
                                if consecutive_failures >= MAX_CONSECUTIVE_ACHIEVEMENT_FAILURES {
                                    warnings.push(format!(
                                        "连续 {MAX_CONSECUTIVE_ACHIEVEMENT_FAILURES} 个条目处理失败（请求或写库），已暂停本轮，稍后再试"
                                    ));
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    consecutive_failures += 1;
                    failed += 1;
                    if failure_samples.len() < 5 {
                        failure_samples.push(format!("{}: {err}", entry.external_id));
                    }
                    let rejected = matches!(err, Error::SourceRejected(_) | Error::RateLimited(_));
                    last_error = Some(err);
                    if rejected {
                        warnings.push(
                            "已触发限流/拒绝，剩余条目稍后再点一次即可（已完成的会保留）"
                                .to_string(),
                        );
                        break 'outer;
                    }
                    // 系统性故障（Key 失效但返回 JSON、接口变更……）时不要空转打满 limit
                    if consecutive_failures >= MAX_CONSECUTIVE_ACHIEVEMENT_FAILURES {
                        warnings.push(format!(
                            "连续 {MAX_CONSECUTIVE_ACHIEVEMENT_FAILURES} 个条目处理失败（请求或写库），已暂停本轮，稍后再试"
                        ));
                        break 'outer;
                    }
                }
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(ACHIEVEMENTS_DELAY_MS)).await;
    }

    let duration_ms = started.elapsed().as_millis() as i64;

    // 收尾步骤失败时也不能把运行记录留在 running：尽力补一条失败记录再上抛
    let pending_after = match state
        .db
        .achievements_pending_count(Source::Steam, Some(account_id))
        .await
    {
        Ok(count) => count,
        Err(err) => {
            finish_run_err_best_effort(&state, run_id, &err.to_string(), duration_ms).await;
            return Err(ApiError(err));
        }
    };

    // 全部失败时按失败收尾（与 enrich 同款处理）：让用户看到「稍后重试」，
    // 而不是一个 0 成功的 200。（unavailable 是正常回答，不算失败。）
    if checked == 0
        && unavailable == 0
        && updated == 0
        && unchanged == 0
        && let Some(err) = last_error
    {
        finish_run_err_best_effort(&state, run_id, &err.to_string(), duration_ms).await;
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

    if let Err(err) = state
        .db
        .finish_run_ok(
            run_id,
            RunCounts {
                seen: checked + unavailable + failed,
                added: 0,
                updated,
                unchanged,
                // skipped 在其它模式里是「被选项过滤」，这里只计真失败；
                // 该账号没有的条目（unavailable）只体现在 warnings 文案里
                skipped: failed,
                missing: 0,
            },
            &warnings,
            duration_ms,
        )
        .await
    {
        finish_run_err_best_effort(&state, run_id, &err.to_string(), duration_ms).await;
        return Err(ApiError(err));
    }

    if let Err(err) = state.db.touch_account_synced(account_id, Utc::now()).await {
        warn!(account_id, error = %err, "刷新账号同步时间失败");
    }

    Ok(Json(AchievementSyncSummary {
        checked,
        updated,
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

/// 收尾失败时尽力把运行记录标记为失败：绝不留一条永远「进行中」的记录。
async fn finish_run_err_best_effort(state: &AppState, run_id: i64, error: &str, duration_ms: i64) {
    if let Err(record_err) = state.db.finish_run_err(run_id, error, duration_ms).await {
        warn!(run_id, error = %record_err, "写入失败记录时又出错了");
    }
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
            // 通用 JSON 导入不建立账号关联：账号来自具体的数据源适配器
            account_id: None,
        })
        .await?;

    let run = state.db.get_import_run(summary.run_id).await?;

    let preview = if request.preview {
        let mut preview = state.db.load_many(&summary.sample_ids).await?;
        state.db.attach_accounts(&mut preview).await?;
        preview
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
        let mut preview = state.db.load_many(&summary.sample_ids).await?;
        state.db.attach_accounts(&mut preview).await?;
        preview
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
