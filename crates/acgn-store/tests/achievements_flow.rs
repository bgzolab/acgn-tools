//! 成就同步的集成测试（sqlite 内存库 + 真实迁移）。
//!
//! 规则：全成就 → 已通关；用户手动标过的状态永不回退；取不到成就时保留上次进度、
//! 只刷新检查时间（轮询按「没查过的优先」推进）。

use acgn_core::{
    AchievementProgress, EntryPatch, ImportOptions, MediaKind, Source, SourceItem, Status,
};
use acgn_store::{Db, ImportRequest, MissingScope};

/// 内存库 + 迁移。
async fn setup() -> Db {
    let db = Db::memory().await.expect("内存库应能创建");
    db.migrate().await.expect("迁移应成功");
    db
}

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

async fn import_library(db: &Db, items: &[SourceItem]) {
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
    .expect("库存导入不应失败");
}

async fn get(db: &Db, appid: &str) -> acgn_core::GameWithEntry {
    db.get_game_by_external(Source::Steam, appid)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("条目 {appid} 应存在"))
}

async fn mark_checked(db: &Db, appid: &str, achieved: i64, total: i64) {
    let id = get(db, appid).await.game.id;
    db.update_achievements(
        id,
        Some(AchievementProgress { achieved, total }),
        chrono::Utc::now(),
    )
    .await
    .expect("写成就进度不应失败");
}

#[tokio::test]
async fn full_achievements_promote_playing_to_completed() {
    let db = setup().await;
    import_library(&db, &[library_item("3001", "Played", 120)]).await;
    assert_eq!(get(&db, "3001").await.entry.status, Status::Playing);

    // 部分成就：状态不变，但进度要落库（UI 展示用）
    mark_checked(&db, "3001", 10, 20).await;
    let entry = get(&db, "3001").await.entry;
    assert_eq!(entry.status, Status::Playing, "没全成就不能改状态");
    assert_eq!(entry.achievements_achieved, Some(10));
    assert_eq!(entry.achievements_total, Some(20));
    assert!(entry.achievements_checked_at.is_some());

    // 全成就：升级为已通关，并补齐完成时间与 100% 进度
    let id = get(&db, "3001").await.game.id;
    let outcome = db
        .update_achievements(
            id,
            Some(AchievementProgress {
                achieved: 20,
                total: 20,
            }),
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    assert!(outcome.promoted, "全成就应触发升级");

    let entry = get(&db, "3001").await.entry;
    assert_eq!(entry.status, Status::Completed);
    assert_eq!(entry.progress_percent, Some(100));
    assert!(entry.finished_at.is_some(), "已通关应有完成时间");
    assert_eq!(entry.playtime_minutes, Some(120), "时长不受影响");

    // 再查一次全成就：已是已通关，不重复触发
    let outcome = db
        .update_achievements(
            id,
            Some(AchievementProgress {
                achieved: 20,
                total: 20,
            }),
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    assert!(!outcome.promoted);
    assert_eq!(get(&db, "3001").await.entry.status, Status::Completed);
}

#[tokio::test]
async fn achievements_never_override_user_states() {
    let db = setup().await;
    import_library(
        &db,
        &[
            library_item("3100", "Dropped", 60),
            library_item("3101", "Ignored", 60),
            library_item("3102", "OnHold", 60),
        ],
    )
    .await;

    for (appid, status) in [
        ("3100", Status::Dropped),
        ("3101", Status::Ignored),
        ("3102", Status::OnHold),
    ] {
        db.update_entry(
            get(&db, appid).await.game.id,
            &EntryPatch {
                status: Some(status),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }

    // 之后即使全成就，也不该把这些用户状态回退成已通关
    for appid in ["3100", "3101", "3102"] {
        mark_checked(&db, appid, 5, 5).await;
    }

    assert_eq!(get(&db, "3100").await.entry.status, Status::Dropped);
    assert_eq!(get(&db, "3101").await.entry.status, Status::Ignored);
    assert_eq!(get(&db, "3102").await.entry.status, Status::OnHold);
    // 进度照样记录，便于展示「全成就（已弃坑）」
    assert_eq!(get(&db, "3100").await.entry.achievements_total, Some(5));
}

#[tokio::test]
async fn unavailable_achievements_keep_progress_and_refresh_checked_at() {
    let db = setup().await;
    import_library(&db, &[library_item("3200", "Private", 30)]).await;
    mark_checked(&db, "3200", 3, 10).await;

    let before = get(&db, "3200").await.entry.achievements_checked_at;
    let id = get(&db, "3200").await.game.id;
    let later = chrono::Utc::now() + chrono::Duration::seconds(5);
    db.update_achievements(id, None, later).await.unwrap();

    let entry = get(&db, "3200").await.entry;
    assert_eq!(entry.achievements_achieved, Some(3), "取不到时保留旧进度");
    assert_eq!(entry.achievements_total, Some(10));
    assert!(entry.achievements_checked_at > before, "检查时间要刷新");
    assert_eq!(entry.status, Status::Playing, "取不到不能改状态");
}

#[tokio::test]
async fn pending_achievements_cover_library_items_and_rotate() {
    let db = setup().await;

    // 愿望单条目（没有来源时长）不在成就候选里
    db.import_items(&ImportRequest {
        source: Source::Steam,
        mode: "steam_wishlist",
        items: &[SourceItem {
            source: Source::Steam,
            external_id: "3300".to_string(),
            media_kind: MediaKind::Game,
            title: "Wishlist Only".to_string(),
            ..Default::default()
        }],
        options: ImportOptions::default(),
        args: None,
        source_mode: "test",
        extra_warnings: Vec::new(),
        missing_scope: MissingScope::Wishlist,
    })
    .await
    .unwrap();

    import_library(
        &db,
        &[
            library_item("3301", "Owned A", 10),
            library_item("3302", "Owned B", 20),
        ],
    )
    .await;

    let pending = db.pending_achievements(Source::Steam, 10).await.unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|entry| entry.external_id.as_str())
            .collect::<Vec<_>>(),
        vec!["3301", "3302"],
        "只挑库存条目，且没查过的在前面"
    );
    assert_eq!(
        db.achievements_pending_count(Source::Steam).await.unwrap(),
        2
    );

    // 查掉一个：它应排到没查过的后面；数量减少
    mark_checked(&db, "3301", 1, 2).await;
    let pending = db.pending_achievements(Source::Steam, 10).await.unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|entry| entry.external_id.as_str())
            .collect::<Vec<_>>(),
        vec!["3302", "3301"],
        "没查过的优先，其次最久没查的"
    );
    assert_eq!(
        db.achievements_pending_count(Source::Steam).await.unwrap(),
        1
    );

    // limit 生效
    assert_eq!(
        db.pending_achievements(Source::Steam, 1)
            .await
            .unwrap()
            .len(),
        1
    );

    // 全部查过：pending_count 归零（列表仍会按轮询返回，用于刷新）
    mark_checked(&db, "3302", 0, 0).await;
    assert_eq!(
        db.achievements_pending_count(Source::Steam).await.unwrap(),
        0
    );
}
