//! `acgn-store`：SQLite 持久化层。
//!
//! * [`Db`] 负责连接、迁移；
//! * `games` / `imports` / `stats` 模块按业务切分，方法都挂在 [`Db`] 上；
//! * 导入合并逻辑（[`Db::import_items`]）是所有数据源的公共入口：
//!   Steam 愿望单、未来的 Bangumi 收藏、小黑盒导入，都走同一条路。

mod accounts;
mod achievements;
mod error;
mod games;
mod imports;
mod rows;
mod stats;

pub mod db;

pub use accounts::{AccountPrice, AccountPriceTarget};
pub use achievements::{AchievementOutcome, PendingAchievement};
pub use db::Db;
pub use games::GameFilters;
pub use imports::{ImportRequest, MetadataOutcome, MissingScope, PendingMetadata, RunCounts};
pub use rows::{GameEntryRow, ImportRunRow};

/// 便于测试与外部构建内存库。
pub use sqlx::sqlite::SqlitePoolOptions;
