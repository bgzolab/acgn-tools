//! `acgn-server` 的库入口：REST API + 静态托管。
//!
//! 提供 lib 目标有两个用途：
//!
//! 1. 集成测试可以直接构造 `Router`（见 `tests/error_envelope.rs`），不必真的起端口；
//! 2. 未来的 Tauri 客户端可以把这套 axum 直接跑在进程内，而不是必须再起一个独立服务。

pub mod config;
pub mod error;
pub mod extract;
pub mod routes;
pub mod state;

pub use error::{ApiError, ApiResult};
pub use state::AppState;
