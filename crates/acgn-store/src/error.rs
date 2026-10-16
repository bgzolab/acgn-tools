//! sqlx 错误 → [`acgn_core::Error`] 的本地扩展 trait。
//!
//! 领域层不能依赖 sqlx（孤儿规则也不允许在 store 里直接 `impl From` 外部类型），
//! 所以用「本地 trait + 泛型实现」的方式提供 `.db()?` 语法。

use acgn_core::{Error, Result};

/// 把 `Result<T, E>` 折叠成 `Result<T, acgn_core::Error>`。
pub trait ResultExt<T> {
    fn db(self) -> Result<T>;
}

impl<T, E> ResultExt<T> for std::result::Result<T, E>
where
    E: std::fmt::Display,
{
    fn db(self) -> Result<T> {
        self.map_err(Error::storage)
    }
}
