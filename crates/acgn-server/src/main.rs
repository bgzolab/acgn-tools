//! `acgn-server`：REST API + （可选）前端静态站点。
//!
//! ```text
//! acgn-server serve    # 启动 HTTP 服务（默认）
//! acgn-server seed     # 导入一次数据后退出
//! acgn-server stats    # 打印数据库概览
//! ```

use acgn_core::{ImportOptions, Source};
use acgn_server::config::AppConfig;
use acgn_server::state::AppState;
use acgn_server::{config as server_config, routes};
use acgn_steam::{SteamClient, SteamConfig};
use acgn_store::{Db, ImportRequest, MissingScope};
use anyhow::Context;
use clap::{Parser, Subcommand};
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "acgn-server",
    version,
    about = "ACGN 管理器后端（游戏优先）：REST API + 静态站点",
    long_about = "配置优先从命令行参数读，其次环境变量：ACGN_BIND / ACGN_DB / ACGN_WEB_DIR / ACGN_LOG / ACGN_STEAM_FIXTURE / STEAM_API_KEY / STEAM_COUNTRY / STEAM_LANGUAGE"
)]
struct Cli {
    /// 监听地址（默认只监听本机；要局域网 / 服务器访问再显式改成 0.0.0.0:8787）
    #[arg(
        long,
        env = "ACGN_BIND",
        default_value = "127.0.0.1:8787",
        global = true
    )]
    bind: String,

    /// SQLite 文件路径
    #[arg(long, env = "ACGN_DB", default_value = "./data/acgn.db", global = true)]
    db: String,

    /// 前端构建产物目录（存在时同端口托管）
    #[arg(
        long,
        env = "ACGN_WEB_DIR",
        default_value = "./web/dist",
        global = true
    )]
    web_dir: String,

    /// 日志过滤规则
    #[arg(
        long,
        env = "ACGN_LOG",
        default_value = "info,acgn_server=debug,acgn_steam=debug,acgn_store=debug",
        global = true
    )]
    log: String,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 启动 HTTP 服务（默认）
    Serve,
    /// 导入一次数据后退出（fixture 模式完全离线）
    Seed {
        /// steamID64 / 个性域名 / 主页链接；不填则用 fixture 里的 steam_id
        #[arg(long)]
        steam_id: Option<String>,
        /// 导入后顺带补全缺失的元数据
        #[arg(long, default_value_t = false)]
        enrich: bool,
        /// 补全条数上限
        #[arg(long, default_value_t = 50)]
        enrich_limit: i64,
    },
    /// 打印数据库概览
    Stats,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(&cli.log);

    let steam_config = SteamConfig::from_env();
    let config = AppConfig {
        bind: cli.bind.clone(),
        db_path: cli.db.clone(),
        web_dir: cli.web_dir.clone(),
        cors_origins: server_config::cors_origins_from_env(),
        steam: steam_config.clone(),
    };

    let db = Db::connect(&config.db_path)
        .await
        .with_context(|| format!("打开数据库失败：{}", config.db_path))?;
    db.migrate().await.context("数据库迁移失败")?;

    let steam = SteamClient::new(steam_config).context("初始化 Steam 客户端失败")?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(config, db, steam).await,
        Command::Seed {
            steam_id,
            enrich,
            enrich_limit,
        } => seed(db, steam, steam_id, enrich, enrich_limit).await,
        Command::Stats => print_stats(&db).await,
    }
}

async fn serve(config: AppConfig, db: Db, steam: SteamClient) -> anyhow::Result<()> {
    let state = AppState::new(db, steam, config.clone());
    let app = routes::build_app(state.clone());

    let listener = TcpListener::bind(&config.bind)
        .await
        .with_context(|| format!("监听 {} 失败", config.bind))?;

    banner(&config, &state);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("已退出");
    Ok(())
}

async fn seed(
    db: Db,
    steam: SteamClient,
    steam_id: Option<String>,
    enrich: bool,
    enrich_limit: i64,
) -> anyhow::Result<()> {
    let input = steam_id
        .or_else(|| steam.fixture().and_then(|fixture| fixture.steam_id.clone()))
        .context("请用 --steam-id 指定 SteamID64 / 个性域名，或在 fixture 里写 steam_id")?;

    let fetch = steam.fetch_wishlist(&input).await?;
    info!(
        steam_id = %fetch.steam_id,
        items = fetch.items.len(),
        mode = fetch.source_mode,
        "已抓取愿望单"
    );

    let summary = db
        .import_items(&ImportRequest {
            source: Source::Steam,
            mode: "steam_wishlist",
            items: &fetch.items,
            options: ImportOptions::default(),
            args: None,
            source_mode: fetch.source_mode,
            extra_warnings: fetch.warnings,
            missing_scope: MissingScope::Wishlist,
        })
        .await?;

    println!(
        "✔ 愿望单导入完成：新增 {} / 更新 {} / 未变 {} / 跳过 {} / 已不在愿望单 {}（{}ms，数据源 {}）",
        summary.added,
        summary.updated,
        summary.unchanged,
        summary.skipped,
        summary.missing,
        summary.duration_ms,
        summary.source_mode
    );
    for warning in &summary.warnings {
        println!("  ⚠ {warning}");
    }

    if enrich {
        let pending = db
            .pending_metadata(Some(Source::Steam), enrich_limit)
            .await?;
        let appids: Vec<String> = pending
            .iter()
            .map(|item| item.external_id.clone())
            .collect();
        println!("→ 补全 {} 个条目的元数据…", appids.len());

        let batch = steam.app_details_batch(&appids).await?;
        let mut ok = 0;
        for item in &batch.items {
            db.update_metadata(item).await?;
            ok += 1;
        }
        println!(
            "  成功 {ok} / 失败 {} / 告警 {} 条",
            appids.len() as i64 - ok,
            batch.warnings.len()
        );
        for warning in batch.warnings.iter().take(8) {
            println!("  ⚠ {warning}");
        }
    }

    Ok(())
}

async fn print_stats(db: &Db) -> anyhow::Result<()> {
    let stats = db.stats().await?;

    println!("总条目：{}", stats.total_games);
    for row in &stats.by_status {
        println!("  {:<8} {}", row.label, row.count);
    }
    if !stats.by_source.is_empty() {
        println!("按来源：");
        for row in &stats.by_source {
            println!("  {:<10} {}", row.label, row.count);
        }
    }
    if let Some(rating) = stats.average_rating {
        println!("平均评分：{rating:.1}");
    }
    println!("总时长：{} 分钟", stats.total_playtime_minutes);
    for bucket in &stats.wishlist_value {
        println!(
            "愿望单价值（{}）：{}.{:02} / 原价 {}.{:02}（{} 个条目）",
            bucket.currency,
            bucket.current_price_cents / 100,
            bucket.current_price_cents % 100,
            bucket.list_price_cents / 100,
            bucket.list_price_cents % 100,
            bucket.item_count
        );
    }
    Ok(())
}

fn banner(config: &AppConfig, state: &AppState) {
    let local = if config.bind.starts_with("0.0.0.0:") {
        format!(
            "http://127.0.0.1:{}",
            config.bind.trim_start_matches("0.0.0.0:")
        )
    } else {
        format!("http://{}", config.bind)
    };

    info!("{} v{}", acgn_core::APP_NAME, acgn_core::VERSION);
    info!(bind = %config.bind, db = %config.db_path, "服务已启动");
    info!("  API      {local}/api");
    info!("  前端     {local}/（需要已构建 web/dist）");
    info!(
        "  Steam    mode={} country={} language={} api_key={}",
        state.steam.mode(),
        state.config.steam.country,
        state.config.steam.language,
        if state.config.steam.has_api_key() {
            "yes"
        } else {
            "no"
        }
    );
    if let Some(fixture) = state.config.steam.fixture.as_ref() {
        info!("  fixture  {}", fixture.display());
    }
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("收到退出信号，正在优雅关闭…");
}
