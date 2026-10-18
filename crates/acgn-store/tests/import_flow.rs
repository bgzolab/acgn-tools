//! 导入合并语义的集成测试（sqlite 内存库 + 真实迁移）。
//!
//! 这些断言就是 README 里「同步语义」那一节的代码化版本：
//! 元数据以来源为准、个人状态以本地为准、消失的条目不删除、重复导入幂等。

use acgn_core::{
    EntryPatch, GameListQuery, ImportOptions, MediaKind, MetadataLevel, RestoreItem, Source,
    SourceItem, Status,
};
use acgn_store::{Db, GameFilters, ImportRequest, MetadataOutcome, MissingScope};
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
        missing_scope: MissingScope::Wishlist,
    })
    .await
    .expect("导入不应失败")
}

/// 仅预览（dry_run）：必须与真实导入同一口径，且不写库。
async fn dry_import(db: &Db, items: &[SourceItem]) -> acgn_core::ImportSummary {
    db.import_items(&ImportRequest {
        source: Source::Steam,
        mode: "steam_wishlist",
        items,
        options: ImportOptions {
            dry_run: true,
            ..Default::default()
        },
        args: None,
        source_mode: "test",
        extra_warnings: Vec::new(),
        missing_scope: MissingScope::Wishlist,
    })
    .await
    .expect("预览不应失败")
}

/// 库存条目（`GetOwnedGames` 的映射结果）：已拥有 + 来源时长。
/// 状态与 `acgn-steam` 的映射一致：有游玩时长 = 在玩，没玩过 = 待玩。
fn library_item(appid: &str, title: &str, playtime_minutes: i64) -> SourceItem {
    SourceItem {
        source: Source::Steam,
        external_id: appid.to_string(),
        media_kind: MediaKind::Game,
        title: title.to_string(),
        status: Some(if playtime_minutes > 0 {
            Status::Playing
        } else {
            Status::Backlog
        }),
        playtime_minutes: Some(playtime_minutes),
        ..Default::default()
    }
}

/// 库存同步（与 `POST /api/imports/steam/library` 同一套参数）。
async fn import_library(db: &Db, items: &[SourceItem]) -> acgn_core::ImportSummary {
    db.import_items(&ImportRequest {
        source: Source::Steam,
        mode: "steam_library",
        items,
        options: ImportOptions::default(),
        args: None,
        source_mode: "test",
        extra_warnings: Vec::new(),
        missing_scope: MissingScope::Library,
    })
    .await
    .expect("库存导入不应失败")
}

/// 库存预览（dry_run）。
async fn dry_import_library(db: &Db, items: &[SourceItem]) -> acgn_core::ImportSummary {
    db.import_items(&ImportRequest {
        source: Source::Steam,
        mode: "steam_library",
        items,
        options: ImportOptions {
            dry_run: true,
            ..Default::default()
        },
        args: None,
        source_mode: "test",
        extra_warnings: Vec::new(),
        missing_scope: MissingScope::Library,
    })
    .await
    .expect("库存预览不应失败")
}

/// 取一条（不存在直接 panic，测试里不需要重复处理 Option）。
async fn get(db: &Db, appid: &str) -> acgn_core::GameWithEntry {
    db.get_game_by_external(Source::Steam, appid)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("条目 {appid} 应存在"))
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
async fn source_status_only_applies_to_unconfirmed_entries() {
    let db = setup().await;

    // 来源声明「已拥有、没玩过」→ 新条目直接落到待玩
    let mut owned = item("200", "Delta");
    owned.status = Some(Status::Backlog);
    import(&db, &[owned]).await;

    let game = db
        .get_game_by_external(Source::Steam, "200")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(game.entry.status, Status::Backlog);

    // 用户标成在玩后，来源再报「待玩」也不能把它回退
    db.update_entry(
        game.game.id,
        &EntryPatch {
            status: Some(Status::Playing),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let mut owned = item("200", "Delta");
    owned.status = Some(Status::Backlog);
    import(&db, &[owned]).await;
    let game = db
        .get_game_by_external(Source::Steam, "200")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(game.entry.status, Status::Playing, "本地状态优先");
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
            missing_scope: MissingScope::Wishlist,
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

/// P1 回归：导出 → 还原必须能把「元数据 + 个人状态」原样带回来，且可重复执行。
#[tokio::test]
async fn restore_round_trip_preserves_state_and_is_idempotent() {
    let source_db = setup().await;
    import(&source_db, &[item("900", "Omicron"), item("901", "Pi")]).await;

    let game = source_db
        .get_game_by_external(Source::Steam, "900")
        .await
        .unwrap()
        .unwrap();

    // 库存同步先建立「来源时长」跟踪（4621 分钟），用户再手填 123 分钟：
    // 还原必须把两个值都带回来。
    import_library(&source_db, &[library_item("900", "Omicron", 4621)]).await;

    source_db
        .update_entry(
            game.game.id,
            &EntryPatch {
                status: Some(Status::Playing),
                rating: Some(9),
                progress_percent: Some(45),
                playtime_minutes: Some(123),
                note: Some("还原要保留这些".to_string()),
                favorite: Some(true),
                priority: Some(2),
            },
        )
        .await
        .unwrap();

    let mut detailed = item("900", "Omicron");
    detailed.metadata_level = MetadataLevel::Full;
    detailed.metadata = Some(serde_json::json!({ "kind": "appdetails" }));
    detailed.short_description = Some("一段简介".to_string());
    detailed.price_final = Some(1999);
    source_db.update_metadata(&detailed).await.unwrap();

    // 导出（就是 GET /api/export/json 的内容）
    let exported = source_db.export_all().await.unwrap();
    assert_eq!(exported.len(), 2);

    // 还原到一个空库
    let target = setup().await;
    let items: Vec<RestoreItem> = exported
        .clone()
        .into_iter()
        .map(RestoreItem::from)
        .collect();
    let summary = target.restore_items(&items, false).await.unwrap();
    assert_eq!(
        (summary.added, summary.updated, summary.unchanged),
        (2, 0, 0)
    );
    assert_eq!(summary.mode, "restore");

    let restored = target
        .get_game_by_external(Source::Steam, "900")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.entry.status, Status::Playing);
    assert_eq!(restored.entry.rating, Some(9));
    assert_eq!(restored.entry.progress_percent, Some(45));
    assert_eq!(restored.entry.playtime_minutes, Some(123));
    assert_eq!(
        restored.entry.source_playtime_minutes,
        Some(4621),
        "来源时长跟踪值也必须随导出/还原往返"
    );
    assert_eq!(restored.entry.note.as_deref(), Some("还原要保留这些"));
    assert!(restored.entry.favorite);
    assert_eq!(restored.entry.priority, Some(2));
    assert_eq!(restored.game.price_final, Some(1999));
    assert_eq!(restored.game.short_description.as_deref(), Some("一段简介"));
    assert!(restored.game.metadata_updated_at.is_some());

    // 再还原一次：幂等（文件内容没变 → unchanged）
    let again = target.restore_items(&items, false).await.unwrap();
    assert_eq!((again.added, again.updated, again.unchanged), (0, 0, 2));

    // 预览不落库、不写运行记录
    let preview = target.restore_items(&items, true).await.unwrap();
    assert_eq!(preview.unchanged, 2);
    assert_eq!(preview.run_id, 0);
}

#[tokio::test]
async fn dry_run_reports_the_same_diff_as_a_real_import() {
    let db = setup().await;
    import(&db, &[item("700", "Kappa"), item("701", "Lambda")]).await;

    // 完全一致 → 预览应报 unchanged=2（以前会误报 updated=2）
    let preview = dry_import(&db, &[item("700", "Kappa"), item("701", "Lambda")]).await;
    assert_eq!(
        (preview.added, preview.updated, preview.unchanged),
        (0, 0, 2)
    );

    // 改了标题 → 预览应报 updated=1，且不落库
    let preview = dry_import(
        &db,
        &[item("700", "Kappa: Remastered"), item("701", "Lambda")],
    )
    .await;
    assert_eq!(
        (preview.added, preview.updated, preview.unchanged),
        (0, 1, 1)
    );
    let untouched = db
        .get_game_by_external(Source::Steam, "700")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(untouched.game.title, "Kappa", "预览不允许改数据");

    // 全新条目 → added=1
    let preview = dry_import(&db, &[item("702", "Mu")]).await;
    assert_eq!(
        (preview.added, preview.updated, preview.unchanged),
        (1, 0, 0)
    );
}

#[tokio::test]
async fn filtered_out_items_are_not_reported_as_missing() {
    let db = setup().await;
    let mut dlc = item("703", "Kappa DLC");
    dlc.media_kind = MediaKind::Dlc;
    import(&db, &[item("700", "Kappa"), dlc.clone()]).await;

    let items = [item("700", "Kappa"), dlc];
    let summary = db
        .import_items(&ImportRequest {
            source: Source::Steam,
            mode: "steam_wishlist",
            items: &items,
            options: ImportOptions {
                include_dlc: false,
                ..Default::default()
            },
            args: None,
            source_mode: "test",
            extra_warnings: Vec::new(),
            missing_scope: MissingScope::Wishlist,
        })
        .await
        .unwrap();

    assert_eq!(summary.skipped, 1, "DLC 应被过滤");
    assert_eq!(
        summary.missing, 0,
        "被过滤的条目仍存在于来源中，不能算 missing"
    );

    // 预览口径一致
    let preview = db
        .import_items(&ImportRequest {
            source: Source::Steam,
            mode: "steam_wishlist",
            items: &items,
            options: ImportOptions {
                include_dlc: false,
                dry_run: true,
                ..Default::default()
            },
            args: None,
            source_mode: "test",
            extra_warnings: Vec::new(),
            missing_scope: MissingScope::Wishlist,
        })
        .await
        .unwrap();
    assert_eq!(preview.skipped, 1);
    assert_eq!(preview.missing, 0);
}

/// 读取 `metadata_json`（原始载荷）方便断言。
async fn metadata_json_of(db: &Db, external_id: &str) -> Option<String> {
    sqlx::query_scalar("SELECT metadata_json FROM games WHERE external_id = ?")
        .bind(external_id)
        .fetch_one(&db.pool)
        .await
        .expect("查询 metadata_json 不应失败")
}

/// P1 回归：`metadata_json` 必须遵守 MetadataLevel 权威规则。
///
/// 否则「导入（列表档 / 愿望单载荷）→ 补全（详情档 / appdetails 载荷）→ 再导入」
/// 会让两种载荷互相改写，每次同步都误报 `updated`，也违反「重复导入必须 no-op」。
#[tokio::test]
async fn list_level_payload_cannot_overwrite_full_level_payload() {
    let db = setup().await;

    let mut wishlist_item = item("810", "Xi");
    wishlist_item.metadata_level = MetadataLevel::List;
    wishlist_item.metadata = Some(serde_json::json!({ "kind": "wishlist", "priority": 3 }));

    let first = import(&db, &[wishlist_item.clone()]).await;
    assert_eq!(first.added, 1);
    let raw = metadata_json_of(&db, "810").await;
    assert!(
        raw.as_deref().unwrap_or_default().contains("wishlist"),
        "列表档应当能「填空」：{raw:?}"
    );

    // 详情档覆盖（appdetails 载荷）
    let mut detailed = item("810", "Xi");
    detailed.metadata_level = MetadataLevel::Full;
    detailed.metadata = Some(serde_json::json!({ "kind": "appdetails", "price_final": 1234 }));
    assert!(matches!(
        db.update_metadata(&detailed).await.unwrap(),
        MetadataOutcome::Updated(_)
    ));

    // 再导入完全相同的列表档：必须 no-op，且不能把载荷改回愿望单版本
    let second = import(&db, &[wishlist_item]).await;
    assert_eq!(second.updated, 0, "列表档不得覆盖详情档的原始载荷");
    assert_eq!(second.unchanged, 1);
    let raw = metadata_json_of(&db, "810").await;
    assert!(
        raw.as_deref().unwrap_or_default().contains("appdetails"),
        "载荷应保持详情档版本：{raw:?}"
    );

    // P2-4 回归：Unchanged 时连时间戳都不该动（否则前端「元数据抓取于…」会一直漂移）
    let before = db
        .get_game_by_external(Source::Steam, "810")
        .await
        .unwrap()
        .unwrap()
        .game
        .metadata_updated_at;
    assert!(matches!(
        db.update_metadata(&detailed).await.unwrap(),
        MetadataOutcome::Unchanged(_)
    ));
    let after = db
        .get_game_by_external(Source::Steam, "810")
        .await
        .unwrap()
        .unwrap()
        .game
        .metadata_updated_at;
    assert_eq!(after, before, "未变化时不应刷新元数据时间戳");
}

#[tokio::test]
async fn metadata_json_is_persisted_and_stable() {
    let db = setup().await;

    let mut with_payload = item("800", "Nu");
    with_payload.metadata = Some(serde_json::json!({ "appid": 800, "priority": 3 }));

    import(&db, &[with_payload.clone()]).await;

    let raw = metadata_json_of(&db, "800").await;
    let raw = raw.expect("metadata_json 必须落库（契约见 dto.rs / appdetails.rs 注释）");
    assert!(raw.contains("\"appid\""), "应保留来源原始载荷：{raw}");

    // 同样的载荷再导入：不应因为 JSON 文本差异被判定为更新
    let summary = import(&db, &[with_payload]).await;
    assert_eq!(summary.updated, 0);
    assert_eq!(summary.unchanged, 1);
}

#[tokio::test]
async fn search_escapes_like_wildcards() {
    let db = setup().await;
    import(
        &db,
        &[item("900", "100% Orange Juice"), item("901", "100 Bullets")],
    )
    .await;

    let page = db
        .list_games(
            &GameFilters {
                q: Some("100%".to_string()),
                ..Default::default()
            },
            None,
            1,
            10,
        )
        .await
        .unwrap();
    assert_eq!(page.total, 1, "`%` 必须当字面量");
    assert_eq!(page.items[0].game.title, "100% Orange Juice");

    let page = db
        .list_games(
            &GameFilters {
                q: Some("_".to_string()),
                ..Default::default()
            },
            None,
            1,
            10,
        )
        .await
        .unwrap();
    assert_eq!(page.total, 0, "`_` 必须当字面量，否则会匹配任意单字符");

    // 字面量下划线仍然搜得到
    let db2 = setup().await;
    import(&db2, &[item("902", "Foo_Bar")]).await;
    let page = db2
        .list_games(
            &GameFilters {
                q: Some("Foo_".to_string()),
                ..Default::default()
            },
            None,
            1,
            10,
        )
        .await
        .unwrap();
    assert_eq!(page.total, 1);
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

// -------------------------------------------------------- 库存导入（已拥有游戏 + 时长）

#[tokio::test]
async fn library_import_status_follows_playtime() {
    let db = setup().await;

    let summary = import_library(
        &db,
        &[
            library_item("1001", "Owned Never Played", 0),
            library_item("1002", "Owned Played", 4621),
        ],
    )
    .await;
    assert_eq!(summary.added, 2, "首次库存导入应新增 2 条");

    let never = get(&db, "1001").await;
    assert_eq!(never.entry.status, Status::Backlog, "没玩过 → 待玩");
    assert_eq!(never.entry.playtime_minutes, Some(0), "0 分钟也是有效时长");
    assert_eq!(never.entry.source_playtime_minutes, Some(0));

    let played = get(&db, "1002").await;
    assert_eq!(played.entry.status, Status::Playing, "有游玩时长 → 在玩");
    assert_eq!(played.entry.playtime_minutes, Some(4621));
    assert_eq!(played.entry.source_playtime_minutes, Some(4621));

    // 幂等：同一份库存重复同步必须 no-op
    let again = import_library(
        &db,
        &[
            library_item("1001", "Owned Never Played", 0),
            library_item("1002", "Owned Played", 4621),
        ],
    )
    .await;
    assert_eq!((again.added, again.updated, again.unchanged), (0, 0, 2));
    assert_eq!(again.missing, 0);
}

/// 来源报「在玩」时，愿望单 / 待玩会被升级；用户手动标过的状态（如已通关）永不回退。
#[tokio::test]
async fn source_playing_promotes_unconfirmed_states_only() {
    let db = setup().await;

    // 愿望单 → 在玩（买到手 + 有时长）
    import(&db, &[item("2001", "Played Later")]).await;
    let summary = import_library(&db, &[library_item("2001", "Played Later", 120)]).await;
    assert_eq!(summary.updated, 1);
    assert_eq!(get(&db, "2001").await.entry.status, Status::Playing);

    // 待玩（0 时长）→ 在玩（第二次同步有时长了）
    import_library(&db, &[library_item("2002", "Owned Not Played", 0)]).await;
    assert_eq!(get(&db, "2002").await.entry.status, Status::Backlog);
    let summary = import_library(&db, &[library_item("2002", "Owned Not Played", 30)]).await;
    assert_eq!(summary.updated, 1);
    let entry = get(&db, "2002").await.entry;
    assert_eq!(entry.status, Status::Playing);
    assert_eq!(entry.playtime_minutes, Some(30));

    // 用户手动标成「已通关」：有时长也不再回退，但时长继续跟随
    db.update_entry(
        get(&db, "2002").await.game.id,
        &EntryPatch {
            status: Some(Status::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    import_library(&db, &[library_item("2002", "Owned Not Played", 60)]).await;
    let entry = get(&db, "2002").await.entry;
    assert_eq!(entry.status, Status::Completed, "手动状态优先");
    assert_eq!(entry.playtime_minutes, Some(60), "时长仍跟随来源");
}

#[tokio::test]
async fn library_playtime_follows_source_until_user_edits() {
    let db = setup().await;
    import_library(&db, &[library_item("1100", "Played", 120)]).await;

    // 时长增加 → 自动跟随；预览与实际导入同一口径，且预览不写库
    let preview = dry_import_library(&db, &[library_item("1100", "Played", 300)]).await;
    assert_eq!(
        (preview.updated, preview.unchanged),
        (1, 0),
        "预览应看到时长变化"
    );
    assert_eq!(
        get(&db, "1100").await.entry.playtime_minutes,
        Some(120),
        "预览不写库"
    );

    let summary = import_library(&db, &[library_item("1100", "Played", 300)]).await;
    assert_eq!(summary.updated, 1);
    assert_eq!(get(&db, "1100").await.entry.playtime_minutes, Some(300));

    // 用户手填（例如主机 / 离线时长）：之后不再被来源覆盖，但来源值继续跟踪
    db.update_entry(
        get(&db, "1100").await.game.id,
        &EntryPatch {
            playtime_minutes: Some(500),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let summary = import_library(&db, &[library_item("1100", "Played", 360)]).await;
    assert_eq!(summary.updated, 1, "来源跟踪值变化仍应报告 updated");
    let entry = get(&db, "1100").await.entry;
    assert_eq!(entry.playtime_minutes, Some(500), "手填值优先");
    assert_eq!(entry.source_playtime_minutes, Some(360));

    // PATCH 负数（清空）= 恢复跟随来源
    let cleared = db
        .update_entry(
            get(&db, "1100").await.game.id,
            &EntryPatch {
                playtime_minutes: Some(-1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(cleared.entry.playtime_minutes, Some(360));

    // 恢复跟随后再同步同一份库存：幂等
    let summary = import_library(&db, &[library_item("1100", "Played", 360)]).await;
    assert_eq!((summary.updated, summary.unchanged), (0, 1));
}

#[tokio::test]
async fn library_import_adopts_wishlist_entries_into_backlog() {
    let db = setup().await;
    import(
        &db,
        &[
            item("1200", "Bought And Playing"),
            item("1201", "Bought Not Started"),
        ],
    )
    .await;

    // 1200 已经被用户标成在玩：库存同步不能把状态打回待玩
    db.update_entry(
        get(&db, "1200").await.game.id,
        &EntryPatch {
            status: Some(Status::Playing),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let summary = import_library(
        &db,
        &[
            library_item("1200", "Bought And Playing", 45),
            library_item("1201", "Bought Not Started", 0),
        ],
    )
    .await;
    assert_eq!(summary.updated, 2);

    let played = get(&db, "1200").await;
    assert_eq!(played.entry.status, Status::Playing, "本地状态必须优先");
    assert_eq!(played.entry.playtime_minutes, Some(45), "时长仍应采纳");

    let fresh = get(&db, "1201").await;
    assert_eq!(
        fresh.entry.status,
        Status::Backlog,
        "仍是愿望单的条目应被挪到待玩"
    );
}

#[tokio::test]
async fn library_missing_scope_is_separate_from_wishlist() {
    let db = setup().await;
    import(&db, &[item("1300", "Wishlist Only")]).await;
    import_library(&db, &[library_item("1301", "Owned A", 10)]).await;

    // 库存里少了 1301：只报库存口径的 1 条；愿望单条目 1300 不归库存管
    let summary = import_library(&db, &[library_item("1302", "Owned B", 5)]).await;
    assert_eq!(summary.missing, 1);
    let warning = summary
        .warnings
        .iter()
        .find(|warning| warning.contains("已不在来源中"))
        .expect("应有 missing 告警");
    assert!(warning.contains("退款"), "应使用库存口径的文案：{warning}");

    // 反过来：愿望单导入只对愿望单负责，库存条目不算 missing
    let summary = import(&db, &[]).await;
    assert_eq!(summary.missing, 1);
    let warning = summary
        .warnings
        .iter()
        .find(|warning| warning.contains("已不在来源中"))
        .expect("应有 missing 告警");
    assert!(
        warning.contains("愿望单"),
        "应使用愿望单口径的文案：{warning}"
    );
}
