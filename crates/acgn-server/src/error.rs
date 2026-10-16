//! HTTP 错误 → JSON 响应。

use acgn_core::Error;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// 统一的 API 错误包装。
#[derive(Debug)]
pub struct ApiError(pub Error);

impl From<Error> for ApiError {
    fn from(value: Error) -> Self {
        ApiError(value)
    }
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match &self.0 {
            Error::Invalid(_) => StatusCode::BAD_REQUEST,
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::SourceRejected(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Error::Network(_) | Error::Upstream(_) => StatusCode::BAD_GATEWAY,
            Error::Storage(_) | Error::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let kind = self.0.kind();
        let hint = self.0.hint();
        let message = self.0.to_string();

        if status.is_server_error() {
            tracing::error!(kind, error = %message, "请求失败");
        } else {
            tracing::warn!(kind, error = %message, "请求被拒绝");
        }

        let body = Json(json!({
            "error": {
                "kind": kind,
                "message": message,
                "hint": hint,
            }
        }));
        (status, body).into_response()
    }
}

/// 便捷别名：handler 统一返回它。
pub type ApiResult<T> = std::result::Result<T, ApiError>;
