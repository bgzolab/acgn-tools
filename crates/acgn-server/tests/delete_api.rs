//! 删除接口的 HTTP 契约测试：状态码、错误包络、批量语义。
//!
//! 存储层语义（级联、事务、与导入的相互作用）在
//! `crates/acgn-store/tests/delete_flow.rs`；这里只钉 HTTP 表面。

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

/// 发一个请求；空响应体（204）折成 `Value::Null`。
async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("请求应完成");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
        .await
        .expect("读取响应体");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("响应体应是 JSON")
    };
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

fn delete(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
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

/// 用扁平 JSON 导入 `count` 条（外部 id = 1..=count，本地 id 也是 1..=count）。
async fn seed(app: &Router, count: usize) {
    let items: Vec<Value> = (1..=count)
        .map(|i| json!({ "external_id": format!("{i}"), "title": format!("Game {i}") }))
        .collect();
    let (status, body) = send(
        app,
        json_request(
            "POST",
            "/api/imports",
            json!({ "source": "steam", "items": items }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "导入应成功：{body}");
    assert_eq!(body["summary"]["added"], count);
}

#[tokio::test]
async fn delete_removes_the_entry_and_is_404_afterwards() {
    let app = test_app().await;
    seed(&app, 2).await;

    let (status, _) = send(&app, delete("/api/games/1")).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "成功删除应返回 204");

    let (status, body) = send(&app, get("/api/games/1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_envelope(&body, "not_found");

    let (status, body) = send(&app, get("/api/games?per_page=50")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1, "只应剩下另一条");
}

#[tokio::test]
async fn delete_unknown_id_uses_the_error_envelope() {
    let app = test_app().await;
    let (status, body) = send(&app, delete("/api/games/42")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_envelope(&body, "not_found");
}

#[tokio::test]
async fn batch_delete_reports_the_real_count() {
    let app = test_app().await;
    seed(&app, 3).await;

    let (status, body) = send(
        &app,
        json_request("POST", "/api/games/delete", json!({ "ids": [1, 2, 999] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], 2, "未知 id 应被忽略：{body}");

    let (_, body) = send(&app, get("/api/games?per_page=50")).await;
    assert_eq!(body["total"], 1);
}

#[tokio::test]
async fn batch_delete_rejects_empty_ids() {
    let app = test_app().await;
    let (status, body) = send(
        &app,
        json_request("POST", "/api/games/delete", json!({ "ids": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_envelope(&body, "invalid");
}

#[tokio::test]
async fn batch_delete_rejects_more_than_1000_ids() {
    let app = test_app().await;
    let ids: Vec<i64> = (1..=1001).collect();
    let (status, body) = send(
        &app,
        json_request("POST", "/api/games/delete", json!({ "ids": ids })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_envelope(&body, "invalid");
}

#[tokio::test]
async fn batch_delete_counts_rows_not_request_ids() {
    let app = test_app().await;
    seed(&app, 1).await;

    let (status, body) = send(
        &app,
        json_request("POST", "/api/games/delete", json!({ "ids": [1, 1] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], 1, "重复 id 不应重复计数：{body}");
}
