//! 游戏新闻的存储语义（内存 SQLite + 真实迁移）。
//!
//! 固化以下约定（对应 `docs/design.md` §5.4）：
//! * 只同步「已拥有」的 Steam 游戏，愿望单不抓；
//! * 同一 guid 重复写入是幂等的，内容变化才会更新；
//! * feed 变短不会删除旧新闻（归档）；
//! * 语言维度隔离，未来切语言不会覆盖当前语言的归档；
//! * 同步状态按游戏记录，失败保留旧新闻。

use acgn_core::{AccountProfile, ImportOptions, NewsEntry, Source, SourceItem, Status};
use acgn_store::{Db, ImportRequest, MissingScope};
use chrono::{Duration, Utc};

async fn setup() -> Db {
    let db = Db::memory().await.expect("内存库应能创建");
    db.migrate().await.expect("迁移应成功");
    db
}

async fn account(db: &Db) -> i64 {
    db.upsert_account(
        Source::Steam,
        "76561198000000000",
        Some(&AccountProfile {
            display_name: Some("Main".to_string()),
            country_code: Some("cn".to_string()),
            ..Default::default()
        }),
    )
    .await
    .expect("账号应创建")
    .0
    .id
}

async fn import(db: &Db, account_id: Option<i64>, scope: MissingScope, items: &[SourceItem]) {
    db.import_items(&ImportRequest {
        source: Source::Steam,
        mode: match scope {
            MissingScope::Wishlist => "steam_wishlist",
            MissingScope::Library => "steam_library",
        },
        items,
        options: ImportOptions::default(),
        args: None,
        source_mode: "test",
        extra_warnings: Vec::new(),
        missing_scope: scope,
        account_id,
    })
    .await
    .expect("导入不应失败");
}

fn library_item(appid: &str, title: &str, playtime: i64) -> SourceItem {
    SourceItem {
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
    }
}

fn wishlist_item(appid: &str, title: &str) -> SourceItem {
    SourceItem {
        source: Source::Steam,
        external_id: appid.to_string(),
        title: title.to_string(),
        ..Default::default()
    }
}

async fn game_id(db: &Db, appid: &str) -> i64 {
    db.get_game_by_external(Source::Steam, appid)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("条目 {appid} 应存在"))
        .game
        .id
}

fn news(guid: &str, title: &str, days_ago: i64) -> NewsEntry {
    NewsEntry {
        guid: guid.to_string(),
        title: title.to_string(),
        summary: Some(format!("summary of {title}")),
        url: format!("https://steamcommunity.com/games/1/announcements/detail/{guid}"),
        author: Some("dev".to_string()),
        published_at: Utc::now() - Duration::days(days_ago),
    }
}

#[tokio::test]
async fn news_targets_only_include_owned_steam_games() {
    let db = setup().await;
    let account_id = account(&db).await;

    import(
        &db,
        Some(account_id),
        MissingScope::Library,
        &[library_item("100", "Owned", 0)],
    )
    .await;
    import(
        &db,
        Some(account_id),
        MissingScope::Wishlist,
        &[wishlist_item("200", "Wishlist only")],
    )
    .await;
    // 旧数据 / 账号删除后：没有 game_accounts，但条目级来源时长仍在
    import(
        &db,
        None,
        MissingScope::Library,
        &[library_item("300", "Legacy owned", 10)],
    )
    .await;

    let targets = db.news_targets("schinese").await.unwrap();
    let ids: Vec<&str> = targets
        .iter()
        .map(|target| target.external_id.as_str())
        .collect();
    assert_eq!(ids, vec!["100", "300"], "愿望单不应抓新闻");

    let wishlist_id = game_id(&db, "200").await;
    assert!(
        db.news_target(wishlist_id).await.unwrap().is_none(),
        "愿望单条目不是新闻目标"
    );
    assert!(
        db.news_target(game_id(&db, "100").await)
            .await
            .unwrap()
            .is_some()
    );

    let status = db.news_status("schinese").await.unwrap();
    assert_eq!(status.owned_games, 2);
    assert_eq!(status.synced_games, 0);
    assert_eq!(status.pending_games, 2);
}

#[tokio::test]
async fn storing_news_is_idempotent_and_keeps_an_archive() {
    let db = setup().await;
    let account_id = account(&db).await;
    import(
        &db,
        Some(account_id),
        MissingScope::Library,
        &[library_item("100", "Owned", 0)],
    )
    .await;
    let id = game_id(&db, "100").await;

    let first = news("1", "Patch 1.0", 2);
    let second = news("2", "Patch 1.1", 1);
    let counts = db
        .store_news(id, "schinese", &[first.clone(), second.clone()], Utc::now())
        .await
        .unwrap();
    assert_eq!((counts.added, counts.updated, counts.unchanged), (2, 0, 0));

    let counts = db
        .store_news(id, "schinese", &[first.clone(), second.clone()], Utc::now())
        .await
        .unwrap();
    assert_eq!(
        (counts.added, counts.updated, counts.unchanged),
        (0, 0, 2),
        "重复抓同一份 feed 必须无变化"
    );

    let mut renamed = second.clone();
    renamed.title = "Patch 1.1 (hotfix)".to_string();
    let counts = db
        .store_news(id, "schinese", &[first.clone(), renamed], Utc::now())
        .await
        .unwrap();
    assert_eq!((counts.added, counts.updated, counts.unchanged), (0, 1, 1));

    // feed 只剩旧的一条：新条目继续留在归档里
    let counts = db
        .store_news(id, "schinese", &[first], Utc::now())
        .await
        .unwrap();
    assert_eq!(counts.unchanged, 1);

    let page = db.list_news("schinese", Some(id), 1, 10).await.unwrap();
    assert_eq!(page.total, 2, "旧新闻不能被 feed 的「最近 N 条」删掉");
    assert_eq!(page.items[0].title, "Patch 1.1 (hotfix)", "按发布时间倒序");
    assert_eq!(page.items[1].title, "Patch 1.0");
    assert_eq!(page.items[0].game_title, "Owned");

    let first_page = db.list_news("schinese", Some(id), 1, 1).await.unwrap();
    assert_eq!(first_page.items.len(), 1);
    assert_eq!(first_page.total_pages, 2);
    assert_eq!(first_page.items[0].title, "Patch 1.1 (hotfix)");
}

#[tokio::test]
async fn languages_are_stored_separately() {
    let db = setup().await;
    let account_id = account(&db).await;
    import(
        &db,
        Some(account_id),
        MissingScope::Library,
        &[library_item("100", "Owned", 0)],
    )
    .await;
    let id = game_id(&db, "100").await;

    let entry = news("1", "公告", 0);
    db.store_news(id, "schinese", std::slice::from_ref(&entry), Utc::now())
        .await
        .unwrap();
    db.store_news(id, "english", std::slice::from_ref(&entry), Utc::now())
        .await
        .unwrap();

    assert_eq!(
        db.list_news("schinese", None, 1, 10).await.unwrap().total,
        1
    );
    assert_eq!(db.list_news("english", None, 1, 10).await.unwrap().total, 1);
    assert_eq!(
        db.list_news("japanese", None, 1, 10).await.unwrap().total,
        0,
        "未同步的语言不应看到其它语言的新闻"
    );
}

#[tokio::test]
async fn status_tracks_success_pending_and_failures() {
    let db = setup().await;
    let account_id = account(&db).await;
    import(
        &db,
        Some(account_id),
        MissingScope::Library,
        &[
            library_item("100", "First", 0),
            library_item("200", "Second", 0),
        ],
    )
    .await;
    let first = game_id(&db, "100").await;
    let second = game_id(&db, "200").await;

    db.store_news(first, "schinese", &[news("1", "First news", 0)], Utc::now())
        .await
        .unwrap();
    db.mark_news_feed_success(first, "schinese", "https://x/1", 1, Utc::now())
        .await
        .unwrap();
    db.mark_news_feed_failure(second, "schinese", "https://x/2", "HTTP 403", Utc::now())
        .await
        .unwrap();

    let status = db.news_status("schinese").await.unwrap();
    assert_eq!(status.owned_games, 2);
    assert_eq!(status.synced_games, 1);
    assert_eq!(status.pending_games, 1);
    assert_eq!(status.failed_games, 1);
    assert_eq!(status.news_items, 1);
    assert!(status.last_synced_at.is_some());

    // 「从没抓过」优先于「抓过但失败」；失败的游戏不会被跳过
    let targets = db.news_targets("schinese").await.unwrap();
    assert_eq!(targets[0].game_id, second);

    // 失败后成功：清掉错误，pending 归零
    db.mark_news_feed_success(second, "schinese", "https://x/2", 0, Utc::now())
        .await
        .unwrap();
    let status = db.news_status("schinese").await.unwrap();
    assert_eq!(status.synced_games, 2);
    assert_eq!(status.pending_games, 0);
    assert_eq!(status.failed_games, 0);
}

#[tokio::test]
async fn huge_page_does_not_overflow() {
    let db = setup().await;
    let page = db
        .list_news("schinese", None, i64::MAX, 100)
        .await
        .expect("超大页码不应 panic / 溢出");
    assert_eq!(page.page, 1_000_000, "页码应被夹到安全上限");
    assert_eq!(page.items.len(), 0);
}

#[tokio::test]
async fn stale_news_runs_are_marked_failed() {
    let db = setup().await;
    let news_run = db
        .begin_run(Source::Steam, "steam_news", None)
        .await
        .expect("新闻运行记录应创建");
    let other_run = db
        .begin_run(Source::Steam, "steam_wishlist", None)
        .await
        .expect("其它运行记录应创建");

    let failed = db
        .fail_stale_news_runs("服务重启，上一轮新闻同步已中断")
        .await
        .expect("清理不应失败");
    assert_eq!(failed, 1, "只应清理 steam_news 的进行中记录");

    let news_run = db.get_import_run(news_run).await.unwrap().unwrap();
    assert_eq!(news_run.status, "failed");
    assert_eq!(
        news_run.error.as_deref(),
        Some("服务重启，上一轮新闻同步已中断")
    );
    assert!(news_run.finished_at.is_some());

    let other_run = db.get_import_run(other_run).await.unwrap().unwrap();
    assert_eq!(other_run.status, "running", "其它模式的运行记录不受影响");
}

#[tokio::test]
async fn deleting_a_game_cascades_news_and_feed_state() {
    let db = setup().await;
    let account_id = account(&db).await;
    import(
        &db,
        Some(account_id),
        MissingScope::Library,
        &[library_item("100", "Owned", 0)],
    )
    .await;
    let id = game_id(&db, "100").await;

    db.store_news(id, "schinese", &[news("1", "News", 0)], Utc::now())
        .await
        .unwrap();
    db.mark_news_feed_success(id, "schinese", "https://x/1", 1, Utc::now())
        .await
        .unwrap();

    assert!(db.delete_game(id).await.unwrap());
    assert_eq!(
        db.list_news("schinese", None, 1, 10).await.unwrap().total,
        0
    );
    let status = db.news_status("schinese").await.unwrap();
    assert_eq!((status.owned_games, status.news_items), (0, 0));

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM news_feeds")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "news_feeds 应随 games 级联删除");
}
