//! 共享状态：数据库 + 数据源客户端 + 配置。

use std::sync::{Arc, Mutex};

use acgn_core::NewsSyncState;
use acgn_steam::SteamClient;
use acgn_store::Db;
use chrono::{DateTime, Utc};

use crate::config::AppConfig;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub steam: Arc<SteamClient>,
    pub config: Arc<AppConfig>,
    pub started_at: DateTime<Utc>,
    /// 新闻全量同步的进度（进程内状态；重启后归零，不影响已入库新闻）。
    pub news_sync: NewsSyncManager,
}

impl AppState {
    pub fn new(db: Db, steam: SteamClient, config: AppConfig) -> Self {
        Self {
            db: Arc::new(db),
            steam: Arc::new(steam),
            config: Arc::new(config),
            started_at: Utc::now(),
            news_sync: NewsSyncManager::default(),
        }
    }

    pub fn uptime_seconds(&self) -> i64 {
        (Utc::now() - self.started_at).num_seconds().max(0)
    }
}

/// 后台新闻同步的进度管理器。
///
/// 一次同步会跑几百个 RSS，请求本身要几十秒到几分钟：接口立即返回、后台任务逐个抓取，
/// 前端轮询 [`NewsSyncState`] 并随时刷新列表。状态在内存里（单进程自托管场景足够），
/// 服务重启后归零；已入库的新闻和 `news_feeds` 不受影响。
#[derive(Debug, Clone, Default)]
pub struct NewsSyncManager {
    inner: Arc<Mutex<NewsSyncState>>,
}

impl NewsSyncManager {
    fn lock(&self) -> std::sync::MutexGuard<'_, NewsSyncState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 当前快照，并补上动态计算的耗时与 ETA。
    pub fn snapshot(&self) -> NewsSyncState {
        let mut state = self.lock().clone();
        let start = state.started_at;
        let end = state.finished_at.unwrap_or_else(Utc::now);
        state.elapsed_ms = start
            .map(|start| (end - start).num_milliseconds().max(0))
            .unwrap_or(0);
        state.eta_seconds = if state.running && state.done > 0 && state.total > state.done {
            let per_item_ms = state.elapsed_ms as f64 / state.done as f64;
            Some(((state.total - state.done) as f64 * per_item_ms / 1000.0).round() as i64)
        } else {
            None
        };
        state
    }

    /// 开始一轮；已经在跑时返回 `false`，调用方应直接返回当前快照。
    pub fn start(&self, total: i64) -> bool {
        let mut state = self.lock();
        if state.running {
            return false;
        }
        *state = NewsSyncState {
            running: true,
            total,
            started_at: Some(Utc::now()),
            ..Default::default()
        };
        true
    }

    pub fn set_current(&self, title: impl Into<String>) {
        self.lock().current_game = Some(title.into());
    }

    pub fn record_success(&self, added: i64, updated: i64, unchanged: i64) {
        let mut state = self.lock();
        state.done += 1;
        state.succeeded += 1;
        state.items_added += added;
        state.items_updated += updated;
        state.items_unchanged += unchanged;
        state.current_game = None;
    }

    pub fn record_failure(&self) {
        let mut state = self.lock();
        state.done += 1;
        state.failed += 1;
        state.current_game = None;
    }

    pub fn push_warning(&self, warning: String) {
        let mut state = self.lock();
        if state.warnings.len() < 12 {
            state.warnings.push(warning);
        }
    }

    /// 收尾：没有目标（未导入库存）时也写入一条可读说明。
    pub fn finish(&self) {
        let mut state = self.lock();
        state.running = false;
        state.finished_at = Some(Utc::now());
        state.current_game = None;
    }

    pub fn finish_empty(&self, warning: String) -> NewsSyncState {
        {
            let mut state = self.lock();
            *state = NewsSyncState {
                running: false,
                finished_at: Some(Utc::now()),
                warnings: vec![warning],
                ..Default::default()
            };
        }
        self.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_tracks_progress_and_finishes() {
        let manager = NewsSyncManager::default();
        assert!(manager.start(3));
        assert!(!manager.start(3), "运行中不能重复启动");

        manager.set_current("Game A");
        manager.record_success(2, 1, 0);
        let state = manager.snapshot();
        assert!(state.running);
        assert_eq!((state.done, state.total, state.succeeded), (1, 3, 1));
        assert_eq!((state.items_added, state.items_updated), (2, 1));
        assert!(state.eta_seconds.is_some(), "已完成一项后应能估算 ETA");

        manager.record_failure();
        manager.finish();
        let state = manager.snapshot();
        assert!(!state.running);
        assert_eq!((state.done, state.failed), (2, 1));
        assert!(state.finished_at.is_some());
        assert!(state.eta_seconds.is_none(), "结束后不再显示 ETA");
    }

    #[test]
    fn finish_empty_resets_the_previous_run() {
        let manager = NewsSyncManager::default();
        assert!(manager.start(2));
        manager.record_success(5, 0, 0);
        manager.finish();

        let state = manager.finish_empty("没有目标".to_string());
        assert!(!state.running);
        assert_eq!((state.total, state.done, state.items_added), (0, 0, 0));
        assert_eq!(state.warnings, vec!["没有目标"]);
    }
}
