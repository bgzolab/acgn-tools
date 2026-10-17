//! 连接与迁移。

use std::path::Path;
use std::time::Duration;

use acgn_core::{Error, Result};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use tracing::info;

use crate::error::ResultExt;

/// 数据库句柄（可克隆，内部是连接池）。
#[derive(Clone, Debug)]
pub struct Db {
    pub pool: SqlitePool,
}

impl Db {
    /// 连接（或创建）指定文件；父目录会自动创建。
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(Error::storage)?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(15));

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(15))
            .connect_with(options)
            .await
            .db()?;

        Ok(Self { pool })
    }

    /// 内存库，仅用于测试（连接数必须为 1，否则每个连接会拿到独立的内存库）。
    pub async fn memory() -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(":memory:")
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .db()?;
        Ok(Self { pool })
    }

    /// 建表 / 升级 schema（`migrations/` 在编译期被打进二进制）。
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await.db()?;
        info!("数据库迁移完成");
        Ok(())
    }

    /// 健康检查。
    pub async fn ping(&self) -> Result<()> {
        sqlx::query_scalar::<_, i64>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .db()?;
        Ok(())
    }
}
