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

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
        .await
        .expect("读取响应体");
    serde_json::from_slice(&bytes).expect("响应体应是 JSON")
}

async fn call(request: Request<Body>) -> (StatusCode, Value) {
    let response = test_app().await.oneshot(request).await.expect("请求应完成");
    let status = response.status();
    (status, body_json(response).await)
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

/// P1 回归：`GET /api/export/json` 的产物必须能在 HTTP 层**原样**还原回去。
#[tokio::test]
async fn export_and_restore_round_trip_over_http() {
    let app = test_app().await;

    // 1) 扁平导入两条
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/imports",
            json!({
                "source": "steam",
                "items": [
                    { "external_id": "100", "title": "Alpha" },
                    { "external_id": "101", "title": "Beta" }
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["summary"]["added"], 2);

    // 2) 写下个人状态（还原时不能丢）
    let response = app
        .clone()
        .oneshot(json_request(
            "PATCH",
            "/api/games/1",
            json!({ "status": "playing", "rating": 8, "note": "round trip" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // 3) 导出
    let response = app.clone().oneshot(get("/api/export/json")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let export = body_json(response).await;
    assert_eq!(export["count"], 2);
    let alpha = export["items"]
        .as_array()
        .expect("items 应是数组")
        .iter()
        .find(|item| item["game"]["external_id"] == "100")
        .expect("导出里应有 Alpha");
    assert_eq!(alpha["entry"]["status"], "playing", "导出必须带上个人状态");
    let items = export["items"].clone();

    // 4) 原样回灌到 restore（本轮 P1 的关键路径）
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/imports/restore",
            json!({ "items": items.clone() }),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "导出文件必须能原样还原：{}",
        body_json(
            app.clone()
                .oneshot(json_request(
                    "POST",
                    "/api/imports/restore",
                    json!({ "items": items.clone() })
                ))
                .await
                .unwrap()
        )
        .await
    );
    let body = body_json(response).await;
    assert_eq!(body["summary"]["unchanged"], 2, "还原到同一个库应幂等");

    // 5) restore 的 dry_run 不落库（run 为 null）
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/imports/restore",
            json!({ "items": items, "dry_run": true }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert!(body["run"].is_null(), "预览不该写运行记录：{body}");
    assert_eq!(body["summary"]["unchanged"], 2);
}

/// 扁平导入接口只接受 `SourceItem`；导出结构走 `/api/imports/restore`（docs/api.md 有说明）。
#[tokio::test]
async fn flat_import_endpoint_rejects_the_export_shape() {
    let (status, body) = call(json_request(
        "POST",
        "/api/imports",
        json!({ "items": [{ "game": { "external_id": "1", "title": "A" }, "entry": null }] }),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_envelope(&body, "invalid");
}
