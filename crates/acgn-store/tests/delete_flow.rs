//! 删除功能的集成测试（sqlite 内存库 + 真实迁移）。
//!
//! 删除是**本地动作**，这里固化它与同步语义的边界：
//! 级联清掉个人状态、不写导入历史、来源里仍在的条目下次导入会重新加回来。

use acgn_core::{ImportOptions, ImportSummary, MediaKind, Source, SourceItem};
use acgn_store::{Db, GameFilters, ImportRequest};

/// 内存库 + 迁移。
async fn setup() -> Db {
    let db = Db::memory().await.expect("内存库应能创建");
    db.migrate().await.expect("迁移应成功");
    db
}

fn item(appid: &str) -> SourceItem {
    SourceItem {
        source: Source::Steam,
        external_id: appid.to_string(),
        media_kind: MediaKind::Game,
        title: format!("Game {appid}"),
        ..Default::default()
    }
}

async fn import(db: &Db, items: &[SourceItem]) -> ImportSummary {
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

async fn game_id(db: &Db, appid: &str) -> i64 {
    db.get_game_by_external(Source::Steam, appid)
        .await
        .unwrap()
        .expect("条目应存在")
        .game
        .id
}

#[tokio::test]
async fn delete_removes_game_and_cascades_to_entry() {
    let db = setup().await;
    import(&db, &[item("1"), item("2")]).await;

    let target = game_id(&db, "1").await;
    assert!(
        db.delete_game(target).await.unwrap(),
        "存在的条目应删除成功"
    );

    assert!(
        db.get_game(target).await.unwrap().is_none(),
        "games 行应删除"
    );

    let orphans: i64 =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM library_entries WHERE game_id = ?")
            .bind(target)
            .fetch_one(&db.pool)
            .await
            .expect("查询应成功");
    assert_eq!(orphans, 0, "个人状态应随外键级联删除");

    let page = db
        .list_games(&GameFilters::default(), None, 1, 50)
        .await
        .unwrap();
    assert_eq!(page.total, 1, "只应剩下另一条");

    // 删除不属于任何一次导入：运行记录不新增
    let runs: i64 = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM import_runs")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(runs, 1, "删除不应写 import_runs");
}

#[tokio::test]
async fn delete_unknown_id_reports_false() {
    let db = setup().await;
    assert!(
        !db.delete_game(42).await.unwrap(),
        "不存在的 id 应返回 false"
    );
}

#[tokio::test]
async fn batch_delete_ignores_unknown_ids() {
    let db = setup().await;
    import(&db, &[item("1"), item("2"), item("3")]).await;

    let (first, second) = (game_id(&db, "1").await, game_id(&db, "2").await);
    let deleted = db.delete_games(&[first, second, 999]).await.unwrap();
    assert_eq!(deleted, 2, "未知 id 应被静默忽略");

    let page = db
        .list_games(&GameFilters::default(), None, 1, 50)
        .await
        .unwrap();
    assert_eq!(page.total, 1, "只应剩下未选中的那条");
}

#[tokio::test]
async fn deleted_item_comes_back_on_next_import() {
    let db = setup().await;
    import(&db, &[item("1")]).await;

    let original = game_id(&db, "1").await;
    assert!(db.delete_game(original).await.unwrap());

    // 来源里还在的条目：删除是单侧动作，下次导入按「新增」重新建出来
    let again = import(&db, &[item("1")]).await;
    assert_eq!(again.added, 1, "删掉后重新导入应记作新增");
    assert_eq!(again.unchanged, 0);

    let restored = game_id(&db, "1").await;
    assert_ne!(restored, original, "重新导入会拿到新的本地 id");
}
