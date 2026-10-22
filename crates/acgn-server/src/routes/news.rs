//! 游戏新闻：已拥有游戏的 Steam 公告 RSS。
//!
//! 设计要点：
//!
//! * 新闻落库、列表只读 SQLite；网络只发生在显式的同步任务里；
//! * 一次同步**拉取全部已拥有游戏**，不再分批：`POST /api/news/sync` 立即返回，
//!   后台任务逐个抓取并实时更新进度，前端轮询 `/api/news/sync/status`；
//! * 每个 feed 抓完就落库，前端可以在同步过程中不断刷新列表（抓到一个更新一份）；
//! * 单个 feed 失败只记 warning 与 `news_feeds.last_error`，不影响其它游戏；
//!   连续失败会熔断本轮，避免代理挂掉时把整批都等超时。

use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use acgn_core::{Error, GameNews, NewsListQuery, NewsStatus, NewsSyncState, Page, Result, Source};
use acgn_steam::news_feed_url;
use acgn_store::{NewsTarget, NewsUpsert, RunCounts};
use axum::Json;
use axum::extract::State;
use chrono::Utc;
use futures::FutureExt;
use serde_json::json;
use tracing::warn;

use crate::error::ApiResult;
use crate::extract::ValidQuery;
use crate::state::AppState;

/// RSS 请求之间的间隔：社区站没有公开限额，但没必要连续打满。
const NEWS_DELAY_MS: u64 = 250;

/// 连续多少个 feed 失败就暂停本轮（系统性故障：代理挂掉、社区站拒绝……）。
const MAX_CONSECUTIVE_FAILURES: usize = 3;

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

/// GET /api/news/sync/status —— 当前（或最近一轮）同步进度。
pub async fn news_sync_status(State(state): State<AppState>) -> Json<NewsSyncState> {
    Json(state.news_sync.snapshot())
}

/// POST /api/news/sync —— 启动一轮「全部已拥有游戏」的新闻同步。
///
/// 请求立即返回；后台任务一边抓取一边落库。已有一轮在跑时返回当前进度，
/// 不会重复启动（前端保持轮询即可）。
pub async fn start_news_sync(State(state): State<AppState>) -> ApiResult<Json<NewsSyncState>> {
    let language = state.config.steam.language.as_str();
    let targets = state.db.news_targets(language).await?;

    if targets.is_empty() {
        return Ok(Json(
            state.news_sync.finish_empty(
                "没有可同步新闻的已拥有 Steam 游戏（先同步库存，或用真实网络 / fixture 重试）"
                    .to_string(),
            ),
        ));
    }

    if !state.news_sync.start(targets.len() as i64) {
        return Ok(Json(state.news_sync.snapshot()));
    }

    let initial = state.news_sync.snapshot();
    let task_state = state.clone();
    tokio::spawn(async move {
        run_news_sync(task_state, targets).await;
    });

    Ok(Json(initial))
}

/// 后台任务：逐个抓取全部新闻源，每完成一个就写库并更新进度。
///
/// 用 `catch_unwind` 包住主循环：无论正常结束还是任务 panic，都会收尾运行记录并
/// 把 `news_sync.running` 复位，避免按钮永远停在「同步中…」。
async fn run_news_sync(state: AppState, targets: Vec<NewsTarget>) {
    let started = Instant::now();
    let language = state.config.steam.language.clone();
    let country = state.config.steam.country.clone();

    let run_id = match state
        .db
        .begin_run(
            Source::Steam,
            "steam_news",
            Some(json!({
                "language": language,
                "country": country,
                "targets": targets.len(),
            })),
        )
        .await
    {
        Ok(run_id) => run_id,
        Err(err) => {
            state
                .news_sync
                .push_warning(format!("无法写入同步记录：{err}"));
            state.news_sync.finish();
            return;
        }
    };

    let outcome = AssertUnwindSafe(run_news_sync_inner(
        &state, &targets, run_id, &language, &country,
    ))
    .catch_unwind()
    .await;

    if outcome.is_err() {
        let error = "新闻同步任务异常终止（任务 panic）";
        state.news_sync.push_warning(error.to_string());
        finish_run_err_best_effort(&state, run_id, error, started.elapsed().as_millis() as i64)
            .await;
    }

    state.news_sync.finish();
}

async fn run_news_sync_inner(
    state: &AppState,
    targets: &[NewsTarget],
    run_id: i64,
    language: &str,
    country: &str,
) {
    let started = Instant::now();
    let mut consecutive_failures = 0usize;
    let mut first_error: Option<String> = None;
    let mut aborted: Option<String> = None;
    let mut warnings = Vec::new();

    for (index, target) in targets.iter().enumerate() {
        state.news_sync.set_current(target.title.clone());
        let feed_url = news_feed_url(&target.external_id, country, language);

        match fetch_and_store(state, target, language, &feed_url).await {
            Ok((counts, feed_warnings)) => {
                consecutive_failures = 0;
                for warning in feed_warnings {
                    push_warning(&mut warnings, warning.clone());
                    state.news_sync.push_warning(warning);
                }
                state
                    .news_sync
                    .record_success(counts.added, counts.updated, counts.unchanged);
            }
            Err(err) if is_rate_limited(&err) => {
                // 429 不记为该游戏的失败：它的上次成功状态/旧新闻保持不变，
                // 下次同步会按原优先级重试；本轮立刻停下，不再打满剩余请求。
                let warning = format!("{}: {err}，已停止本轮", target.title);
                push_warning(&mut warnings, warning.clone());
                state.news_sync.push_warning(warning);
                aborted = Some(err.to_string());
                break;
            }
            Err(err) => {
                consecutive_failures += 1;
                state.news_sync.record_failure();
                let warning = format!("{}: {err}", target.title);
                push_warning(&mut warnings, warning.clone());
                state.news_sync.push_warning(warning);
                first_error.get_or_insert_with(|| err.to_string());
                record_feed_failure(state, target, language, &feed_url, &err).await;

                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    let warning = format!(
                        "连续 {MAX_CONSECUTIVE_FAILURES} 个新闻源失败，已暂停本轮（稍后再试）"
                    );
                    push_warning(&mut warnings, warning.clone());
                    state.news_sync.push_warning(warning);
                    break;
                }
            }
        }

        if state.steam.mode() != "fixture" && index + 1 < targets.len() {
            tokio::time::sleep(Duration::from_millis(NEWS_DELAY_MS)).await;
        }
    }

    let duration_ms = started.elapsed().as_millis() as i64;
    let snapshot = state.news_sync.snapshot();
    let counts = RunCounts {
        seen: snapshot.done,
        added: snapshot.items_added,
        updated: snapshot.items_updated,
        unchanged: snapshot.items_unchanged,
        skipped: snapshot.failed,
        missing: 0,
    };

    if snapshot.succeeded == 0 && (snapshot.failed > 0 || aborted.is_some()) {
        let reason = aborted
            .or(first_error)
            .unwrap_or_else(|| "所有新闻源请求失败".to_string());
        finish_run_err_best_effort(state, run_id, &reason, duration_ms).await;
    } else if let Err(err) = state
        .db
        .finish_run_ok(run_id, counts, &warnings, duration_ms)
        .await
    {
        finish_run_err_best_effort(state, run_id, &err.to_string(), duration_ms).await;
    }
}

/// 是否是 Steam 的 429 限流：这类错误应该终止本轮，而不是把游戏记成失败。
fn is_rate_limited(err: &Error) -> bool {
    matches!(err, Error::SourceRejected(message) if message.contains("429") || message.contains("限流"))
}

/// 抓取 + 落库一个 feed；状态记账失败只记 warning，不影响新闻入库。
async fn fetch_and_store(
    state: &AppState,
    target: &NewsTarget,
    language: &str,
    feed_url: &str,
) -> Result<(NewsUpsert, Vec<String>)> {
    let fetch = state.steam.fetch_news(&target.external_id).await?;
    let counts = state
        .db
        .store_news(target.game_id, language, &fetch.entries, Utc::now())
        .await?;

    let mut warnings = fetch
        .warnings
        .into_iter()
        .map(|warning| format!("{}: {warning}", target.title))
        .collect::<Vec<_>>();
    if let Err(err) = state
        .db
        .mark_news_feed_success(
            target.game_id,
            language,
            feed_url,
            fetch.entries.len() as i64,
            Utc::now(),
        )
        .await
    {
        // 新闻已经落库，状态记账失败不该让整轮同步失败。
        warnings.push(format!("{}: 记录同步状态失败（{err}）", target.title));
    }
    Ok((counts, warnings))
}

/// 记录 feed 失败；状态写失败只影响下次排序，不阻塞本轮。
async fn record_feed_failure(
    state: &AppState,
    target: &NewsTarget,
    language: &str,
    feed_url: &str,
    error: &Error,
) {
    if let Err(err) = state
        .db
        .mark_news_feed_failure(
            target.game_id,
            language,
            feed_url,
            &error.to_string(),
            Utc::now(),
        )
        .await
    {
        warn!(game_id = target.game_id, error = %err, "记录新闻源失败状态时出错");
    }
}

/// 告警只保留前 12 条：几百个游戏全失败时不要把运行记录撑爆。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_rate_limit_errors() {
        assert!(is_rate_limited(&Error::rejected(
            "Steam 新闻 RSS 触发限流（HTTP 429），请稍后再同步"
        )));
        assert!(!is_rate_limited(&Error::upstream(
            "新闻 RSS 解析失败: 没有 RSS"
        )));
        assert!(!is_rate_limited(&Error::network("请求超时")));
    }
}
