//! 导入合并语义的集成测试（sqlite 内存库 + 真实迁移）。
//!
//! 这些断言就是 README 里「同步语义」那一节的代码化版本：
//! 元数据以来源为准、个人状态以本地为准、消失的条目不删除、重复导入幂等。

use acgn_core::{
    EntryPatch, GameListQuery, ImportOptions, MediaKind, MetadataLevel, Source, SourceItem, Status,
};
use acgn_store::{Db, GameFilters, ImportRequest, MetadataOutcome};
use chrono::Utc;

/// 内存库 + 迁移。
async fn setup() -> Db {
    let db = Db::memory().await.expect("内存库应能创建");
    db.migrate().await.expect("迁移应成功");
    db
}

/// 固定时间戳（带亚微秒精度），用来验证「精度损失不会误判为有变化」。
fn fixed_added_at() -> chrono::DateTime<Utc> {
    chrono::TimeZone::timestamp_opt(&Utc, 1_760_000_000, 123_456_789)
        .single()
        .expect("固定时间戳应合法")
}

fn item(appid: &str, title: &str) -> SourceItem {
    SourceItem {
        source: Source::Steam,
        external_id: appid.to_string(),
        media_kind: MediaKind::Game,
        title: title.to_string(),
        cover_url: Some(format!("https://example.com/{appid}.jpg")),
        review_score: Some(8),
        review_desc: Some("Very Positive".to_string()),
        release_raw: Some("16 May, 2011".to_string()),
        release_date: Some("2011-05-16".to_string()),
        priority: Some(1),
        source_added_at: Some(fixed_added_at()),
        ..Default::default()
    }
}

async fn import(db: &Db, items: &[SourceItem]) -> acgn_core::ImportSummary {
    db.import_items(&ImportRequest {
        source: Source::Steam,
        mode: "steam_wishlist",
        items,
        options: ImportOptions::default(),
        args: None,
        source_mode: "test",
        extra_warnings: Vec::new(),
    })
    .await
    .expect("导入不应失败")
}

#[tokio::test]
async fn import_is_idempotent_and_updates_metadata() {
    let db = setup().await;

    let first = import(&db, &[item("1", "Alpha"), item("2", "Beta")]).await;
    assert_eq!(first.added, 2, "首次导入应新增 2 条");
    assert_eq!(first.updated, 0);

    // 重复导入：不应产生新增，也不该判定为更新
    let second = import(&db, &[item("1", "Alpha"), item("2", "Beta")]).await;
    assert_eq!(second.added, 0);
    assert_eq!(second.updated, 0, "完全相同的元数据不应算更新");
    assert_eq!(second.unchanged, 2);

    // 元数据变化：应判定为更新，且标题被来源覆盖
    let mut renamed = item("1", "Alpha: Remastered");
    renamed.review_score = Some(9);
    let third = import(&db, &[renamed]).await;
    assert_eq!(third.updated, 1);
    // 这次只带了一条，另一条应被记为「已不在来源中」
    assert_eq!(third.missing, 1);
    assert!(
        third
            .warnings
            .iter()
            .any(|warning| warning.contains("已不在来源中"))
    );

    let game = db
        .get_game_by_external(Source::Steam, "1")
        .await
        .unwrap()
        .expect("条目存在");
    assert_eq!(game.game.title, "Alpha: Remastered");
    assert_eq!(game.game.review_score, Some(9));

    // 消失的条目没有被删除
    let page = db
        .list_games(&GameFilters::default(), None, 1, 50)
        .await
        .unwrap();
    assert_eq!(page.total, 2, "消失的条目应保留在本地");
}

#[tokio::test]
async fn user_status_and_notes_are_never_overwritten_by_import() {
    let db = setup().await;
    import(&db, &[item("100", "Gamma")]).await;

    let game = db
        .get_game_by_external(Source::Steam, "100")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(game.entry.status, Status::Wishlist);

    // 用户标成「在玩」并写备注
    let updated = db
        .update_entry(
            game.game.id,
            &EntryPatch {
                status: Some(Status::Playing),
                rating: Some(9),
                progress_percent: Some(40),
                note: Some("第三章".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.entry.status, Status::Playing);
    assert!(
        updated.entry.started_at.is_some(),
        "切到在玩应自动补开始时间"
    );

    // 再同步一次愿望单
    let summary = import(&db, &[item("100", "Gamma")]).await;
    assert_eq!(summary.updated, 0);
    assert_eq!(summary.unchanged, 1);

    let after = db
        .get_game_by_external(Source::Steam, "100")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.entry.status, Status::Playing, "状态必须保持本地值");
    assert_eq!(after.entry.rating, Some(9));
    assert_eq!(after.entry.note.as_deref(), Some("第三章"));
    assert_eq!(after.entry.progress_percent, Some(40));
    assert!(after.entry.last_synced_at.is_some(), "应刷新同步时间");
}

#[tokio::test]
async fn source_status_is_adopted_only_while_still_wishlist() {
    let db = setup().await;

    // 来源声明「已拥有」→ 新条目直接落到待玩
    let mut owned = item("200", "Delta");
    owned.status = Some(Status::Backlog);
    import(&db, &[owned]).await;

    let game = db
        .get_game_by_external(Source::Steam, "200")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(game.entry.status, Status::Backlog);
}

#[tokio::test]
async fn metadata_enrichment_marks_and_clears_pending() {
    let db = setup().await;
    import(&db, &[item("300", "Epsilon")]).await;

    // 列表级导入后应处于「待补全」
    let pending = db.pending_metadata(Some(Source::Steam), 10).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].external_id, "300");

    let mut detailed = item("300", "Epsilon");
    detailed.short_description = Some("一段简介".to_string());
    detailed.price_currency = Some("CNY".to_string());
    detailed.price_initial = Some(29800);
    detailed.price_final = Some(19900);
    detailed.discount_percent = Some(33);

    let outcome = db.update_metadata(&detailed).await.unwrap();
    assert!(matches!(outcome, MetadataOutcome::Updated(_)));

    let game = db
        .get_game_by_external(Source::Steam, "300")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(game.game.price_final, Some(19900));
    assert_eq!(game.game.discount_percent, Some(33));
    assert!(game.game.metadata_updated_at.is_some());

    // 补全后不再出现在待补全列表里
    let pending = db.pending_metadata(Some(Source::Steam), 10).await.unwrap();
    assert!(pending.is_empty());
}

#[tokio::test]
async fn list_level_metadata_never_clobbers_full_metadata() {
    let db = setup().await;
    import(&db, &[item("600", "Iota")]).await;

    // 详情档：正式封面（带 hash）+ 简介 + 价格 + 中文名
    let mut detailed = item("600", "Iota");
    detailed.metadata_level = MetadataLevel::Full;
    detailed.cover_url = Some(
        "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/600/header.jpg?t=1"
            .to_string(),
    );
    detailed.poster_url = Some(
        "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/600/library_600x900.jpg?t=1"
            .to_string(),
    );
    detailed.short_description = Some("一段简介".to_string());
    detailed.title_zh = Some("约塔".to_string());
    detailed.price_final = Some(1234);
    // 详情档会给出本地化的发售日期文案（列表档是英文）
    detailed.release_raw = Some("2011 年 5 月 16 日".to_string());
    detailed.release_date = Some("2011-05-16".to_string());
    assert!(matches!(
        db.update_metadata(&detailed).await.unwrap(),
        MetadataOutcome::Updated(_)
    ));

    // 再同步列表档：封面不能被改回通用 URL，也不能判定成「有变化」
    let summary = import(&db, &[item("600", "Iota")]).await;
    assert_eq!(summary.updated, 0, "列表档不应覆盖详情档的封面");
    assert_eq!(summary.unchanged, 1);
    let game = db
        .get_game_by_external(Source::Steam, "600")
        .await
        .unwrap()
        .unwrap();
    assert!(game.game.cover_url.as_deref().unwrap().contains("akamai"));
    assert_eq!(game.game.price_final, Some(1234));
    assert_eq!(game.game.title_zh.as_deref(), Some("约塔"));
    assert_eq!(game.game.short_description.as_deref(), Some("一段简介"));
    assert_eq!(
        game.game.release_raw.as_deref(),
        Some("2011 年 5 月 16 日"),
        "本地化的发售日文案也不应被列表档改回英文"
    );

    // 详情档再抓一次：内容相同，应判定为未变化
    assert!(matches!(
        db.update_metadata(&detailed).await.unwrap(),
        MetadataOutcome::Unchanged(_)
    ));
}

#[tokio::test]
async fn dry_run_reports_without_writing() {
    let db = setup().await;

    let summary = db
        .import_items(&ImportRequest {
            source: Source::Steam,
            mode: "steam_wishlist",
            items: &[item("400", "Zeta")],
            options: ImportOptions {
                dry_run: true,
                ..Default::default()
            },
            args: None,
            source_mode: "test",
            extra_warnings: Vec::new(),
        })
        .await
        .unwrap();

    assert_eq!(summary.added, 1);
    assert_eq!(summary.run_id, 0, "预览不落库");

    let page = db
        .list_games(&GameFilters::default(), None, 1, 10)
        .await
        .unwrap();
    assert_eq!(page.total, 0, "预览不应写入任何数据");
}

#[tokio::test]
async fn list_filters_and_stats_work() {
    let db = setup().await;
    import(&db, &[item("500", "Eta"), item("501", "Theta")]).await;

    let game = db
        .get_game_by_external(Source::Steam, "500")
        .await
        .unwrap()
        .unwrap();
    db.update_entry(
        game.game.id,
        &EntryPatch {
            status: Some(Status::Playing),
            playtime_minutes: Some(120),
            favorite: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let filters = GameFilters::from_query(&GameListQuery {
        status: Some(Status::Playing),
        ..Default::default()
    });
    let page = db
        .list_games(&filters, Some("playtime_desc"), 1, 10)
        .await
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].game.title, "Eta");

    let stats = db.stats().await.unwrap();
    assert_eq!(stats.total_games, 2);
    assert_eq!(stats.favorite_count, 1);
    assert_eq!(stats.total_playtime_minutes, 120);
    assert_eq!(
        stats
            .by_status
            .iter()
            .find(|row| row.status == Status::Playing)
            .map(|row| row.count),
        Some(1)
    );
    assert!(!stats.recent_imports.is_empty(), "应记录导入历史");
}
