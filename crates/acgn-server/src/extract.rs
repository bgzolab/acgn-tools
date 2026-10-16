//! 自定义提取器：把 axum 默认的 rejection 折成项目统一的错误包络。
//!
//! 约定见 `AGENTS.md` §4.7：所有错误都必须是 `{"error": {kind, message, hint}}`。
//! axum 对 `Json` / `Path` / `Query` 的提取失败默认返回纯文本 400/422，前端的统一错误
//! 处理（`web/src/api/http.ts`）拿不到 `kind` / `hint`，所以这里统一包一层。

use acgn_core::Error;
use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

/// `Json<T>` 的统一错误版本。
pub struct ValidJson<T>(pub T);

impl<T, S> FromRequest<S> for ValidJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(ValidJson(value)),
            Err(rejection) => Err(ApiError(Error::invalid(rejection.body_text()))),
        }
    }
}

/// `Path<T>` 的统一错误版本（例如 `/api/games/abc`）。
pub struct ValidPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ValidPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(ValidPath(value)),
            Err(rejection) => Err(ApiError(Error::invalid(rejection.body_text()))),
        }
    }
}

/// `Query<T>` 的统一错误版本（例如 `?status=nope`）。
pub struct ValidQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ValidQuery<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(ValidQuery(value)),
            Err(rejection) => Err(ApiError(Error::invalid(rejection.body_text()))),
        }
    }
}
