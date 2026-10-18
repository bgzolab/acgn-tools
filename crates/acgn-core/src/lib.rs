//! `acgn-core`：整个项目的领域层。
//!
//! 这里只放纯数据结构和共享契约，不依赖数据库、网络、HTTP 框架。
//! 目标：
//!
//! * `acgn-store`（SQLite 持久化）、`acgn-steam`（Steam 数据源）都只依赖它；
//! * 未来的 Tauri 客户端可以复用同一套模型，不需要改契约；
//! * 未来接入 Bangumi / 小黑盒时，只要产出 [`SourceItem`]，就能复用导入与同步逻辑。
//!
//! [`SourceItem`]: model::SourceItem

pub mod dto;
pub mod error;
pub mod model;

pub use dto::*;
pub use error::{Error, Result};
pub use model::*;

/// 应用名，用于日志与 API 元信息。
pub const APP_NAME: &str = "acgn-tools";

/// 当前 crate 版本（跟随 workspace）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 所有数据源共享的协议版本，未来做多端同步时用于兼容性判断。
///
/// * v1 → v2：`LibraryEntry` 新增 `source_playtime_minutes`（Steam 库存时长跟踪）。
///   旧导出文件仍可还原（缺失字段视为「没有来源值」），新导出给旧客户端会忽略该字段。
pub const SCHEMA_REVISION: i64 = 2;

/// `POST /api/games/delete` 一次最多接受的条目数（HTTP 层校验，见 `docs/api.md`）。
pub const MAX_BATCH_DELETE: usize = 1000;
