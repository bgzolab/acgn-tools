//! 共享状态：数据库 + 数据源客户端 + 配置。

use std::sync::Arc;

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
}

impl AppState {
    pub fn new(db: Db, steam: SteamClient, config: AppConfig) -> Self {
        Self {
            db: Arc::new(db),
            steam: Arc::new(steam),
            config: Arc::new(config),
            started_at: Utc::now(),
        }
    }

    pub fn uptime_seconds(&self) -> i64 {
        (Utc::now() - self.started_at).num_seconds().max(0)
    }
}
