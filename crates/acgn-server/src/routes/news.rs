//! 游戏新闻：已拥有游戏的 Steam 公告 RSS。
//!
//! 设计要点：
//!
//! * 新闻落库、列表只读 SQLite；网络只发生在显式的 `POST /api/news/sync`；
//! * 一次同步处理一小批游戏（默认 30），按「从没抓过 → 抓得最久」轮转，
//!   大型库存可以多点几次，或在 UI 里连续点「继续同步」；
//! * 单个 feed 失败只记 warning 与 `news_feeds.last_error`，不影响其它游戏；
//!   连续失败会熔断本轮，避免代理挂掉时把整批都等超时。

use std::time::Instant;

use acgn_core::{Error, GameNews, NewsListQuery, NewsStatus, NewsSyncSummary, Page, Source};
use acgn_steam::news_feed_url;
use acgn_store::RunCounts;
use axum::Json;
use axum::extract::State;
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use tracing::warn;

use crate::error::{ApiError, ApiResult};
use crate::extract::{ValidJson, ValidQuery};
use crate::state::AppState;

/// 一次同步默认 / 最多处理多少个游戏。
const DEFAULT_LIMIT: i64 = 30;
const MAX_LIMIT: i64 = 200;

/// RSS 请求之间的间隔：社区站没有公开限额，但没必要连续打满。
const NEWS_DELAY_MS: u64 = 250;

/// 连续多少个 feed 失败就暂停本轮（系统性故障：代理挂掉、社区站拒绝……）。
const MAX_CONSECUTIVE_FAILURES: usize = 3;

/// POST /api/news/sync
#[derive(Debug, Default, Deserialize)]
pub struct NewsSyncRequest {
    /// 本次最多同步多少个游戏（默认 30，最大 200）
    #[serde(default)]
    pub limit: Option<i64>,
    /// 只同步一个游戏（详情抽屉 / 手动重试）
    #[serde(default)]
    pub game_id: Option<i64>,
}

/// GET /api/news
pub async fn list_news(
    State(state): State<AppState>,
    ValidQuery(query): ValidQuery<NewsListQuery>,
) -> ApiResult<Json<Page<GameNews>>> {
    let language = state.config.steam.language.as_str();
    let page = state
        .db
        .list_news(language, query.game_id, query.page(), query.per_page())
        .await?;
    Ok(Json(page))
}

/// GET /api/news/status
pub async fn news_status(State(state): State<AppState>) -> ApiResult<Json<NewsStatus>> {
    let language = state.config.steam.language.as_str();
    Ok(Json(state.db.news_status(language).await?))
}

/// POST /api/news/sync —— 抓取下一批已拥有游戏的新闻。
pub async fn sync_news(
    State(state): State<AppState>,
    ValidJson(request): ValidJson<NewsSyncRequest>,
) -> ApiResult<Json<NewsSyncSummary>> {
    let started = Instant::now();
    let language = state.config.steam.language.clone();
    let country = state.config.steam.country.clone();
    let limit = request.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let targets = match request.game_id {
        Some(game_id) => match state.db.news_target(game_id).await? {
            Some(target) => vec![target],
            None => {
                return Err(ApiError(Error::invalid(format!(
                    "游戏 #{game_id} 不在已拥有的 Steam 库中，无法同步新闻"
                ))));
            }
        },
        None => state.db.news_targets(&language, limit).await?,
    };

    if targets.is_empty() {
        let status = state.db.news_status(&language).await?;
        return Ok(Json(NewsSyncSummary {
            run_id: 0,
            games_checked: 0,
            games_failed: 0,
            items_added: 0,
            items_updated: 0,
            items_unchanged: 0,
            pending_games: status.pending_games,
            source_mode: state.steam.mode().to_string(),
            duration_ms: started.elapsed().as_millis() as i64,
            warnings: vec![
                "没有可同步新闻的已拥有 Steam 游戏（先同步库存，或用真实网络 / fixture 重试）"
                    .to_string(),
            ],
        }));
    }

    let run_id = state
        .db
        .begin_run(
            Source::Steam,
            "steam_news",
            Some(json!({
                "language": language,
                "country": country,
                "limit": limit,
                "game_id": request.game_id,
            })),
        )
        .await?;

    let mut games_checked = 0i64;
    let mut games_failed = 0i64;
    let mut items_added = 0i64;
    let mut items_updated = 0i64;
    let mut items_unchanged = 0i64;
    let mut consecutive_failures = 0usize;
    let mut first_error: Option<String> = None;
    let mut warnings = Vec::new();

    for (index, target) in targets.iter().enumerate() {
        let feed_url = news_feed_url(&target.external_id, &country, &language);

        match state.steam.fetch_news(&target.external_id).await {
            Ok(fetch) => {
                consecutive_failures = 0;
                for warning in &fetch.warnings {
                    push_warning(&mut warnings, warning.clone());
                }

                match state
                    .db
                    .store_news(target.game_id, &language, &fetch.entries, Utc::now())
                    .await
                {
                    Ok(counts) => {
                        games_checked += 1;
                        items_added += counts.added;
                        items_updated += counts.updated;
                        items_unchanged += counts.unchanged;

                        if let Err(err) = state
                            .db
                            .mark_news_feed_success(
                                target.game_id,
                                &language,
                                &feed_url,
                                fetch.entries.len() as i64,
                                Utc::now(),
                            )
                            .await
                        {
                            // 新闻已经落库，状态记账失败不该让整轮同步失败。
                            push_warning(
                                &mut warnings,
                                format!("{}: 记录同步状态失败（{err}）", target.title),
                            );
                        }
                    }
                    Err(err) => {
                        games_failed += 1;
                        consecutive_failures += 1;
                        record_failure(
                            &state,
                            target.game_id,
                            &language,
                            &feed_url,
                            &err.to_string(),
                        )
                        .await;
                        push_warning(&mut warnings, format!("{}: {err}", target.title));
                        first_error.get_or_insert_with(|| err.to_string());

                        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                            push_warning(
                                &mut warnings,
                                format!(
                                    "连续 {MAX_CONSECUTIVE_FAILURES} 个新闻源处理失败，已暂停本轮（稍后再试）"
                                ),
                            );
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                games_failed += 1;
                consecutive_failures += 1;
                push_warning(&mut warnings, format!("{}: {err}", target.title));
                first_error.get_or_insert_with(|| err.to_string());
                record_failure(
                    &state,
                    target.game_id,
                    &language,
                    &feed_url,
                    &err.to_string(),
                )
                .await;

                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    push_warning(
                        &mut warnings,
                        format!(
                            "连续 {MAX_CONSECUTIVE_FAILURES} 个新闻源失败，已暂停本轮（稍后再试）"
                        ),
                    );
                    break;
                }
            }
        }

        if state.steam.mode() != "fixture" && index + 1 < targets.len() {
            tokio::time::sleep(std::time::Duration::from_millis(NEWS_DELAY_MS)).await;
        }
    }

    let duration_ms = started.elapsed().as_millis() as i64;
    let pending_games = state
        .db
        .news_status(&language)
        .await
        .map(|status| status.pending_games)
        .unwrap_or(0);

    // 全部失败时把运行记录标成 failed：至少让同步历史诚实地反映这一轮没有成功。
    if games_checked == 0 && games_failed > 0 {
        let reason = first_error.unwrap_or_else(|| "所有新闻源请求失败".to_string());
        finish_run_err_best_effort(&state, run_id, &reason, duration_ms).await;
    } else if let Err(err) = state
        .db
        .finish_run_ok(
            run_id,
            RunCounts {
                seen: games_checked + games_failed,
                added: items_added,
                updated: items_updated,
                unchanged: items_unchanged,
                skipped: games_failed,
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

    Ok(Json(NewsSyncSummary {
        run_id,
        games_checked,
        games_failed,
        items_added,
        items_updated,
        items_unchanged,
        pending_games,
        source_mode: state.steam.mode().to_string(),
        duration_ms,
        warnings,
    }))
}

/// 记录 feed 失败；状态写失败只影响下次排序，不阻塞本轮。
async fn record_failure(
    state: &AppState,
    game_id: i64,
    language: &str,
    feed_url: &str,
    error: &str,
) {
    if let Err(err) = state
        .db
        .mark_news_feed_failure(game_id, language, feed_url, error, Utc::now())
        .await
    {
        warn!(game_id, error = %err, "记录新闻源失败状态时出错");
    }
}

/// 告警只保留前 12 条：几百个游戏全失败时不要把响应撑爆。
fn push_warning(warnings: &mut Vec<String>, warning: String) {
    if warnings.len() < 12 {
        warnings.push(warning);
    }
}

async fn finish_run_err_best_effort(state: &AppState, run_id: i64, error: &str, duration_ms: i64) {
    if let Err(record_err) = state.db.finish_run_err(run_id, error, duration_ms).await {
        warn!(run_id, error = %record_err, "写入失败记录时又出错了");
    }
}
