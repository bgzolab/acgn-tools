//! 统一错误类型。
//!
//! 各层（storage / data source / http）都把自己的错误折叠到 [`Error`]，
//! 上层（HTTP 层、未来的 Tauri command）只需要匹配一次。

use std::fmt;

/// 全项目共享的错误类型。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 输入不合法（用户填错 SteamID、参数越界……）
    #[error("输入不合法: {0}")]
    Invalid(String),

    /// 目标不存在
    #[error("找不到资源: {0}")]
    NotFound(String),

    /// HTTP / 网络层错误
    #[error("网络请求失败: {0}")]
    Network(String),

    /// 上游返回了我们无法理解的载荷
    #[error("上游数据解析失败: {0}")]
    Upstream(String),

    /// 上游明确拒绝（限流、隐私设置、权限……）
    #[error("数据源拒绝: {0}")]
    SourceRejected(String),

    /// 上游触发限流（HTTP 429）：调用方应停止当前批处理，稍后再试
    #[error("触发限流: {0}")]
    RateLimited(String),

    /// 存储层错误
    #[error("存储错误: {0}")]
    Storage(String),

    /// 兜底
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// 语法糖：`Result<T, acgn_core::Error>`。
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// 该错误是否值得让调用方重试（用于导入/补全时的容错判断）。
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Error::Network(_) | Error::SourceRejected(_) | Error::RateLimited(_) | Error::Other(_)
        )
    }

    /// 给前端展示的建议动作，避免前端硬编码字符串匹配。
    pub fn hint(&self) -> &'static str {
        match self {
            Error::Invalid(_) => "请检查输入",
            Error::NotFound(_) => "资源不存在或已被删除",
            Error::Network(_) => "检查服务器出网/代理设置",
            Error::Upstream(_) => "上游接口可能已变更，请反馈",
            Error::SourceRejected(_) => "可能是隐私设置或触发了限流，稍后重试",
            Error::RateLimited(_) => "稍后重试",
            Error::Storage(_) => "数据库异常，检查磁盘与文件权限",
            Error::Other(_) => "未知错误",
        }
    }

    /// 便于在日志里快速定位来源。
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Invalid(_) => "invalid",
            Error::NotFound(_) => "not_found",
            Error::Network(_) => "network",
            Error::Upstream(_) => "upstream",
            Error::SourceRejected(_) => "source_rejected",
            Error::RateLimited(_) => "rate_limited",
            Error::Storage(_) => "storage",
            Error::Other(_) => "other",
        }
    }
}

impl Error {
    /// 存储层错误（由 `acgn-store` 的扩展 trait 调用，避免领域层依赖 sqlx）。
    pub fn storage(err: impl fmt::Display) -> Self {
        Error::Storage(err.to_string())
    }

    /// 网络层错误（由 `acgn-steam` 的扩展 trait 调用，避免领域层依赖 reqwest）。
    pub fn network(err: impl fmt::Display) -> Self {
        Error::Network(err.to_string())
    }

    /// 上游载荷错误。
    pub fn upstream(err: impl fmt::Display) -> Self {
        Error::Upstream(err.to_string())
    }

    pub fn invalid(err: impl fmt::Display) -> Self {
        Error::Invalid(err.to_string())
    }

    pub fn not_found(err: impl fmt::Display) -> Self {
        Error::NotFound(err.to_string())
    }

    pub fn rejected(err: impl fmt::Display) -> Self {
        Error::SourceRejected(err.to_string())
    }

    /// 上游限流（HTTP 429）：调用方应停止当前批处理，稍后重试。
    pub fn rate_limited(err: impl fmt::Display) -> Self {
        Error::RateLimited(err.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Error::Upstream(format!("JSON 解析失败: {value}"))
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Error::Storage(format!("IO 错误: {value}"))
    }
}
