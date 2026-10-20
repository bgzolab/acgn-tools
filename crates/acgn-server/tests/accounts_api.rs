//! 账号接口的契约测试：列表 / 添加（fixture，不联网）/ 区域修改 / 删除 / 按账号筛选。

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

/// 一个只含最小 fixture 的临时目录（解析账号资料不联网）。
fn fixture_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("acgn-accounts-api-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时 fixture 目录");
    std::fs::write(
        dir.join("wishlist.json"),
        r#"{"steam_id":"76561198000000000","wishlist":{}}"#,
    )
    .expect("写 wishlist fixture");
    std::fs::write(
        dir.join("profile.json"),
        r#"{
            "steam_id": "76561198000000000",
            "player": {
                "steamid": "76561198000000000",
                "personaname": "Fixture Player",
                "profileurl": "https://steamcommunity.com/profiles/76561198000000000/",
                "avatarfull": "https://avatars.example/f.jpg",
                "loccountrycode": "US"
            }
        }"#,
    )
    .expect("写 profile fixture");
    std::fs::write(
        dir.join("appdetails.json"),
        r#"{
            "100": { "success": true, "data": { "name": "Owned", "type": "game" } },
            "300": {
                "success": true,
                "data": {
                    "name": "Regional",
                    "type": "game",
                    "is_free": false,
                    "price_overview": {
                        "currency": "USD",
                        "initial": 2999,
                        "final": 1499,
                        "discount_percent": 50
                    }
                }
            }
        }"#,
    )
    .expect("写 appdetails fixture");
    dir
}

async fn test_app(tag: &str) -> (Router, Db) {
    let db = Db::memory().await.expect("内存库应能创建");
    db.migrate().await.expect("迁移应成功");

    let fixture = fixture_dir(tag);
    let steam_config = SteamConfig {
        fixture: Some(fixture.clone()),
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

async fn seed_library(db: &Db, account_id: i64, appid: &str, title: &str, playtime: i64) {
    let item = SourceItem {
        source: Source::Steam,
        external_id: appid.to_string(),
        title: title.to_string(),
        status: Some(if playtime > 0 {
            Status::Playing
        } else {
            Status::Backlog
        }),
        playtime_minutes: Some(playtime),
        ..Default::default()
    };
    db.import_items(&ImportRequest {
        source: Source::Steam,
        mode: "steam_library",
        items: &[item],
        options: ImportOptions::default(),
        args: None,
        source_mode: "test",
        extra_warnings: Vec::new(),
        missing_scope: MissingScope::Library,
        account_id: Some(account_id),
    })
    .await
    .expect("库存导入不应失败");
}

#[tokio::test]
async fn resolve_creates_once_and_lists_counts() {
    let (app, db) = test_app("resolve").await;

    // 空输入 → 统一错误包络
    let (status, body) = send(
        &app,
        json_request("POST", "/api/accounts/resolve", &json!({ "input": "  " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["kind"], "invalid");

    // 不支持的数据源
    let (status, body) = send(
        &app,
        json_request(
            "POST",
            "/api/accounts/resolve",
            &json!({ "source": "bangumi", "input": "123" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // fixture 模式：昵称 / 头像 / 区域都来自 profile.json
    let (status, body) = send(
        &app,
        json_request(
            "POST",
            "/api/accounts/resolve",
            &json!({ "input": "76561198000000000" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["created"], true);
    assert_eq!(body["resolved_via"], "numeric");
    assert_eq!(body["account"]["display_name"], "Fixture Player");
    assert_eq!(body["account"]["effective_country"], "us");
    assert_eq!(body["account"]["external_id"], "76561198000000000");

    // 再解析一次：不新建
    let (status, body) = send(
        &app,
        json_request(
            "POST",
            "/api/accounts/resolve",
            &json!({ "input": "76561198000000000" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["created"], false);

    let account_id = body["account"]["id"].as_i64().unwrap();
    seed_library(&db, account_id, "100", "Owned", 30).await;

    let (status, body) = send(&app, get("/api/accounts")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["owned_count"], 1);
    assert_eq!(body[0]["wishlist_count"], 0);
    assert!(body[0]["last_synced_at"].is_string());
}

#[tokio::test]
async fn countries_are_validated_and_deleting_an_account_keeps_games() {
    let (app, db) = test_app("country").await;
    let (account, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000001",
            Some(&AccountProfile {
                display_name: Some("A".to_string()),
                country_code: Some("cn".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    seed_library(&db, account.id, "200", "Kept", 10).await;

    let (status, body) = send(
        &app,
        json_request(
            "PATCH",
            &format!("/api/accounts/{}", account.id),
            &json!({ "country_code": "USA" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = send(
        &app,
        json_request(
            "PATCH",
            &format!("/api/accounts/{}", account.id),
            &json!({ "country_code": "US" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["country_override"], "us");
    assert_eq!(body["effective_country"], "us");

    // 空串 = 跟随资料
    let (status, body) = send(
        &app,
        json_request(
            "PATCH",
            &format!("/api/accounts/{}", account.id),
            &json!({ "country_code": "" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["effective_country"], "cn");

    // 按账号筛选 + 条目带上账号关联
    let (status, body) = send(&app, get(&format!("/api/games?account_id={}", account.id))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["accounts"][0]["account_id"], account.id);

    // PATCH 条目的响应也要带 accounts（前端会直接替换列表对象）
    let game_id = body["items"][0]["game"]["id"].as_i64().unwrap();
    let (status, body) = send(
        &app,
        json_request(
            "PATCH",
            &format!("/api/games/{game_id}"),
            &json!({ "favorite": true }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["accounts"][0]["account_id"], account.id);

    // 删除账号：204 → 再删 404；游戏仍在
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/accounts/{}", account.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let (status, _) = send(
        &app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/accounts/{}", account.id))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = send(&app, get(&format!("/api/games/{game_id}"))).await;
    assert_eq!(status, StatusCode::OK, "删除账号不应删除游戏");
    assert!(
        body.get("accounts").is_none()
            || body["accounts"]
                .as_array()
                .is_some_and(|items| items.is_empty()),
        "账号关联应已清空：{body}"
    );
}

#[tokio::test]
async fn enrich_fills_prices_for_the_account_region() {
    let (app, db) = test_app("enrich").await;
    let (account, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000002",
            Some(&AccountProfile {
                display_name: Some("US".to_string()),
                country_code: Some("us".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    seed_library(&db, account.id, "300", "Regional", 10).await;

    // 全局区域是 cn，账号区域是 us：补全时两段都要跑
    let (status, body) = send(
        &app,
        json_request("POST", "/api/enrich", &json!({ "limit": 20 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["account_prices"], 1,
        "应按账号区域写入一条价格：{body}"
    );

    let (status, body) = send(&app, get(&format!("/api/games?account_id={}", account.id))).await;
    assert_eq!(status, StatusCode::OK);
    let link = &body["items"][0]["accounts"][0];
    assert_eq!(link["price_currency"], "USD");
    assert_eq!(link["price_initial"], 2999);
    assert_eq!(link["price_final"], 1499);
    assert_eq!(link["discount_percent"], 50);
    assert!(link["price_updated_at"].is_string());

    // 再补一次：没有待补条目，也不会重复写价格
    let (status, body) = send(
        &app,
        json_request("POST", "/api/enrich", &json!({ "limit": 20 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["account_prices"], 0);
}
