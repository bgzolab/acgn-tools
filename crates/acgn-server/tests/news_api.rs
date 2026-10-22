//! 游戏新闻接口契约：列表 / 覆盖率 / 后台全量同步（fixture，不联网）。

use std::path::PathBuf;

use acgn_core::{AccountProfile, ImportOptions, Source, SourceItem, Status};
use acgn_server::config::AppConfig;
use acgn_server::routes::build_app;
use acgn_server::state::AppState;
use acgn_steam::{SteamClient, SteamConfig};
use acgn_store::{Db, ImportRequest, MissingScope};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

fn fixture_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("acgn-news-api-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时 fixture 目录");
    std::fs::write(
        dir.join("wishlist.json"),
        r#"{"steam_id":"76561198000000000","wishlist":{}}"#,
    )
    .expect("写 wishlist fixture");
    std::fs::write(
        dir.join("news.json"),
        r#"{
            "note": "测试新闻",
            "news": {
                "100": [
                    {
                        "guid": "https://steamcommunity.com/games/100/announcements/detail/2",
                        "title": "Newer patch",
                        "summary": "Second announcement",
                        "url": "https://steamcommunity.com/games/100/announcements/detail/2",
                        "author": "dev",
                        "published_at": "2026-10-20T12:00:00Z"
                    },
                    {
                        "guid": "https://steamcommunity.com/games/100/announcements/detail/1",
                        "title": "Older patch",
                        "summary": "First announcement",
                        "url": "https://steamcommunity.com/games/100/announcements/detail/1",
                        "author": "dev",
                        "published_at": "2026-10-01T12:00:00Z"
                    }
                ]
            }
        }"#,
    )
    .expect("写 news fixture");
    dir
}

async fn test_app(tag: &str) -> (Router, Db) {
    let db = Db::memory().await.expect("内存库应能创建");
    db.migrate().await.expect("迁移应成功");

    let fixture = fixture_dir(tag);
    let steam_config = SteamConfig {
        fixture: Some(fixture),
        ..Default::default()
    };
    let config = AppConfig {
        bind: "127.0.0.1:0".to_string(),
        db_path: ":memory:".to_string(),
        web_dir: "/nonexistent-web".to_string(),
        cors_origins: Vec::new(),
        steam: steam_config.clone(),
    };
    let steam = SteamClient::new(steam_config).expect("Steam 客户端应能创建");
    let app = build_app(AppState::new(db.clone(), steam, config));
    (app, db)
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
        .await
        .expect("读取响应体");
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).expect("响应体应是 JSON")
}

fn json_request(method: &str, uri: &str, body: &Value) -> Request<Body> {
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

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("请求应完成");
    let status = response.status();
    (status, body_json(response).await)
}

async fn seed(db: &Db) -> (i64, i64, i64) {
    let (account, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000000",
            Some(&AccountProfile {
                display_name: Some("Main".to_string()),
                country_code: Some("cn".to_string()),
                ..Default::default()
            }),
        )
        .await
        .expect("账号应创建");
    let account_id = account.id;

    db.import_items(&ImportRequest {
        source: Source::Steam,
        mode: "steam_library",
        items: &[SourceItem {
            source: Source::Steam,
            external_id: "100".to_string(),
            title: "Owned".to_string(),
            status: Some(Status::Backlog),
            playtime_minutes: Some(0),
            ..Default::default()
        }],
        options: ImportOptions::default(),
        args: None,
        source_mode: "test",
        extra_warnings: Vec::new(),
        missing_scope: MissingScope::Library,
        account_id: Some(account_id),
    })
    .await
    .expect("库存导入不应失败");

    db.import_items(&ImportRequest {
        source: Source::Steam,
        mode: "steam_wishlist",
        items: &[SourceItem {
            source: Source::Steam,
            external_id: "200".to_string(),
            title: "Wishlist only".to_string(),
            ..Default::default()
        }],
        options: ImportOptions::default(),
        args: None,
        source_mode: "test",
        extra_warnings: Vec::new(),
        missing_scope: MissingScope::Wishlist,
        account_id: Some(account_id),
    })
    .await
    .expect("愿望单导入不应失败");

    let owned = db
        .get_game_by_external(Source::Steam, "100")
        .await
        .unwrap()
        .unwrap()
        .game
        .id;
    let wishlist = db
        .get_game_by_external(Source::Steam, "200")
        .await
        .unwrap()
        .unwrap()
        .game
        .id;
    (account_id, owned, wishlist)
}

async fn wait_for_sync(app: &Router) -> Value {
    for _ in 0..300 {
        let (status, body) = send(app, get("/api/news/sync/status")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if body["running"] == false && body["started_at"].is_string() {
            return body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("新闻同步任务在 3 秒内没有结束");
}

#[tokio::test]
async fn sync_and_list_news_for_owned_games_only() {
    let (app, db) = test_app("flow").await;
    let (_, owned, _) = seed(&db).await;

    let (status, body) = send(&app, get("/api/news")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 0);

    let (status, body) = send(&app, json_request("POST", "/api/news/sync", &json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["running"], true);
    assert_eq!(body["total"], 1, "愿望单游戏不应被抓取：{body}");
    assert_eq!(body["done"], 0);

    let body = wait_for_sync(&app).await;
    assert_eq!(body["done"], 1);
    assert_eq!(body["succeeded"], 1);
    assert_eq!(body["failed"], 0);
    assert_eq!(body["items_added"], 2);
    assert!(body["finished_at"].is_string());
    assert!(body["eta_seconds"].is_null(), "结束后不再估算 ETA");

    let (status, body) = send(&app, get("/api/news")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 2);
    assert_eq!(body["items"][0]["title"], "Newer patch", "按发布时间倒序");
    assert_eq!(body["items"][0]["game_title"], "Owned");
    assert_eq!(body["items"][0]["external_id"], "100");
    assert_eq!(body["items"][0]["language"], "schinese");
    assert_eq!(body["items"][0]["summary"], "Second announcement");
    assert_eq!(body["items"][1]["title"], "Older patch");

    // 分页
    let (status, body) = send(&app, get("/api/news?page=1&per_page=1")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 2);
    assert_eq!(body["total_pages"], 2);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);

    let (status, body) = send(&app, get(&format!("/api/news?game_id={owned}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 2);

    let (status, body) = send(&app, get("/api/news/status")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["owned_games"], 1);
    assert_eq!(body["synced_games"], 1);
    assert_eq!(body["pending_games"], 0);
    assert_eq!(body["failed_games"], 0);
    assert_eq!(body["news_items"], 2);
    assert!(body["last_synced_at"].is_string());

    // 再同步一次：同一份 fixture 必须 unchanged，不重复插入
    let (status, body) = send(&app, json_request("POST", "/api/news/sync", &json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let body = wait_for_sync(&app).await;
    assert_eq!(body["items_added"], 0);
    assert_eq!(body["items_unchanged"], 2);

    let (status, body) = send(&app, get("/api/news")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 2);

    let (status, body) = send(&app, get("/api/imports?limit=1")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["mode"], "steam_news");
    assert_eq!(body[0]["seen"], 1);
    assert_eq!(body[0]["unchanged"], 2);
}

#[tokio::test]
async fn sync_without_owned_games_reports_a_warning() {
    let (app, _db) = test_app("empty").await;
    let (status, body) = send(&app, json_request("POST", "/api/news/sync", &json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["running"], false);
    assert_eq!(body["total"], 0);
    assert_eq!(body["started_at"], Value::Null, "没有目标时不会启动任务");
    assert!(
        body["warnings"][0]
            .as_str()
            .is_some_and(|warning| warning.contains("库存")),
        "{body}"
    );
}
