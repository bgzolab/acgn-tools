//! 路由装配。
//!
//! 约定：所有接口挂在 `/api` 下；前端构建产物存在时同一端口托管静态站点，
//! 未来的 Tauri 客户端要么内置这套 axum（进程内调用），要么直接打这个 HTTP 端口。

pub mod enrich;
pub mod games;
pub mod health;
pub mod imports;
pub mod misc;
pub mod stats;

use axum::Json;
use axum::Router;
use axum::http::{HeaderValue, Method, header};
use axum::routing::{any, get, post};
use serde_json::json;
use tower_http::cors::CorsLayer;

use crate::config::AppConfig;
use crate::state::AppState;

/// `/api` 下的全部接口。
pub fn api_router(state: AppState) -> Router<AppState> {
    Router::new()
        // 元信息
        .route("/", get(api_index))
        .route("/health", get(health::health))
        .route("/config", get(misc::config))
        .route("/stats", get(stats::stats))
        .route("/export/json", get(misc::export_json))
        // 游戏
        .route("/games", get(games::list_games))
        .route("/games/{id}", get(games::get_game).patch(games::patch_game))
        .route(
            "/games/{id}/refresh-metadata",
            post(games::refresh_metadata),
        )
        // 导入 / 同步
        .route(
            "/imports",
            get(imports::list_runs).post(imports::import_json),
        )
        .route("/imports/{id}", get(imports::get_run))
        .route(
            "/imports/steam/wishlist",
            post(imports::import_steam_wishlist),
        )
        .route("/imports/restore", post(imports::restore_json))
        .route("/enrich", post(enrich::enrich))
        // 兜底：未知的 /api 路径返回 JSON，而不是前端的 index.html
        .route("/{*rest}", any(api_not_found))
        .with_state(state)
}

async fn api_index() -> Json<serde_json::Value> {
    Json(json!({
        "name": acgn_core::APP_NAME,
        "version": acgn_core::VERSION,
        "endpoints": [
            { "method": "GET",   "path": "/api/health",                    "desc": "健康检查" },
            { "method": "GET",   "path": "/api/config",                    "desc": "前端配置（状态枚举、数据源模式）" },
            { "method": "GET",   "path": "/api/stats",                     "desc": "看板统计" },
            { "method": "GET",   "path": "/api/games",                     "desc": "列表：status/source/q/sort/page/per_page/favorite" },
            { "method": "GET",   "path": "/api/games/{id}",                "desc": "详情" },
            { "method": "PATCH", "path": "/api/games/{id}",                "desc": "改状态/评分/进度/备注/收藏" },
            { "method": "POST",  "path": "/api/games/{id}/refresh-metadata", "desc": "抓取该条目的商店详情" },
            { "method": "POST",  "path": "/api/imports/steam/wishlist",    "desc": "导入 Steam 愿望单" },
            { "method": "POST",  "path": "/api/imports",                   "desc": "通用 JSON 导入（扁平 SourceItem，合并语义）" },
            { "method": "POST",  "path": "/api/imports/restore",           "desc": "备份还原（导出文件原样回灌，文件优先）" },
            { "method": "GET",   "path": "/api/imports",                   "desc": "导入历史" },
            { "method": "POST",  "path": "/api/enrich",                    "desc": "批量补全缺失的元数据" },
            { "method": "GET",   "path": "/api/export/json",               "desc": "全量导出（备份 / 未来同步）" }
        ]
    }))
}

async fn api_not_found() -> (axum::http::StatusCode, Json<serde_json::Value>) {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(
            json!({ "error": { "kind": "not_found", "message": "接口不存在", "hint": "见 GET /api" } }),
        ),
    )
}

/// 组装完整应用：API + （可选）前端静态站点。
pub fn build_app(state: AppState) -> Router {
    let index_path = std::path::Path::new(&state.config.web_dir).join("index.html");
    let api = api_router(state.clone());

    let mut app = Router::new().nest("/api", api);

    if index_path.exists() {
        let serve_dir = tower_http::services::ServeDir::new(&state.config.web_dir)
            .not_found_service(tower_http::services::ServeFile::new(index_path));
        app = app.fallback_service(serve_dir);
        tracing::info!(web_dir = %state.config.web_dir, "已托管前端静态站点");
    } else {
        app = app.fallback(web_not_built);
        tracing::warn!(
            web_dir = %state.config.web_dir,
            "未找到前端构建产物，仅提供 API（开发时用 pnpm dev，或 pnpm build 后重启）"
        );
    }

    app.layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(cors_layer(&state.config))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

/// CORS：只放行配置里的来源。
///
/// 同源部署（acgn-server 同时托管前端）与 Vite 开发代理都不经过 CORS，
/// 所以默认白名单只留本机开发端口；要跨站调用再通过 `ACGN_CORS_ORIGINS` 显式放行。
fn cors_layer(config: &AppConfig) -> CorsLayer {
    let mut origins: Vec<HeaderValue> = Vec::new();
    for origin in &config.cors_origins {
        match origin.parse::<HeaderValue>() {
            Ok(value) => origins.push(value),
            Err(_) => tracing::warn!(origin, "忽略非法的 CORS 来源配置"),
        }
    }

    if origins.is_empty() {
        tracing::info!("CORS 白名单为空：只接受同源请求");
        return CorsLayer::new();
    }

    tracing::info!(origins = ?config.cors_origins, "CORS 白名单");
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE])
}

async fn web_not_built() -> (axum::http::StatusCode, Json<serde_json::Value>) {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(json!({
            "message": "后端已就绪，但前端尚未构建",
            "how_to": [
                "开发模式：cd web && pnpm install && pnpm dev（Vite 会代理 /api 到本服务）",
                "生产模式：cd web && pnpm build，然后重启 acgn-server"
            ],
            "api_index": "/api"
        })),
    )
}
