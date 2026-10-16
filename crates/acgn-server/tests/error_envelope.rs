//! 服务端错误契约的回归测试。
//!
//! `AGENTS.md` §4.7 规定所有失败都必须是 `{"error": {kind, message, hint}}`；
//! axum 默认的 `Json` / `Path` / `Query` rejection 会返回纯文本，这里确保我们自己的
//! `ValidJson` / `ValidPath` / `ValidQuery` 把它折成了统一包络。

use acgn_server::config::AppConfig;
use acgn_server::routes::build_app;
use acgn_server::state::AppState;
use acgn_steam::{SteamClient, SteamConfig};
use acgn_store::Db;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn test_app() -> Router {
    let db = Db::memory().await.expect("内存库应能创建");
    db.migrate().await.expect("迁移应成功");

    let steam_config = SteamConfig::default();
    let config = AppConfig {
        bind: "127.0.0.1:0".to_string(),
        db_path: ":memory:".to_string(),
        web_dir: "/nonexistent-web".to_string(),
        cors_origins: vec!["http://localhost:5173".to_string()],
        steam: steam_config.clone(),
    };
    let steam = SteamClient::new(steam_config).expect("Steam 客户端应能创建");

    build_app(AppState::new(db, steam, config))
}

async fn call(request: Request<Body>) -> (StatusCode, Value) {
    let response = test_app().await.oneshot(request).await.expect("请求应完成");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
        .await
        .expect("读取响应体");
    let body: Value = serde_json::from_slice(&bytes).expect("响应体应是 JSON");
    (status, body)
}

fn json_request(method: &str, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("构造请求")
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("构造请求")
}

fn assert_envelope(body: &Value, kind: &str) {
    assert_eq!(body["error"]["kind"], kind, "错误包络不符合契约：{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|m| !m.is_empty()),
        "message 必须是非空字符串：{body}"
    );
    assert!(
        body["error"]["hint"].is_string(),
        "hint 必须存在（前端靠它给建议动作）：{body}"
    );
}

#[tokio::test]
async fn invalid_query_enum_uses_the_error_envelope() {
    let (status, body) = call(get("/api/games?status=nope")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_envelope(&body, "invalid");
}

#[tokio::test]
async fn invalid_json_body_uses_the_error_envelope() {
    let (status, body) = call(json_request(
        "PATCH",
        "/api/games/1",
        json!({ "status": "nope" }),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_envelope(&body, "invalid");
}

#[tokio::test]
async fn invalid_path_param_uses_the_error_envelope() {
    let (status, body) = call(get("/api/games/abc")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_envelope(&body, "invalid");
}

#[tokio::test]
async fn missing_json_fields_use_the_error_envelope() {
    // items 是必填字段：缺失时也必须是统一包络，而不是 axum 的纯文本
    let (status, body) = call(json_request("POST", "/api/imports", json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_envelope(&body, "invalid");
}

#[tokio::test]
async fn mixed_source_json_import_is_rejected() {
    let body = json!({
        "items": [
            { "source": "steam", "external_id": "1", "title": "A" },
            { "source": "bangumi", "external_id": "2", "title": "B" }
        ]
    });
    let (status, body) = call(json_request("POST", "/api/imports", body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_envelope(&body, "invalid");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("混用")),
        "应明确提示混源：{body}"
    );
}

#[tokio::test]
async fn unknown_api_route_returns_json_404() {
    let (status, body) = call(get("/api/nope")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_envelope(&body, "not_found");
}
