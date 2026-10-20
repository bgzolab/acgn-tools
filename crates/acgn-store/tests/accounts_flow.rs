//! 账号体系 + 多人库存关联的集成测试（内存 SQLite + 真实迁移）。
//!
//! 覆盖的约定（与 `docs/design.md` §4.6 对应）：
//! * 同一个 `(source, external_id)` 只有一条账号记录，重复抓资料是幂等的；
//! * 库存 / 愿望单按账号建立关联，missing 口径按账号各自统计；
//! * 多账号拥有同一个游戏时，有效时长取所有账号的最大值，小号同步不会覆盖主号；
//! * 按账号筛选、条目附带关联；删除账号不删除游戏；
//! * 区域价格按账号保存，改区域会清掉旧区域的价格缓存。

use acgn_core::{
    AccountProfile, AccountSummary, EntryPatch, GameListQuery, ImportOptions, Source, SourceItem,
    Status,
};
use acgn_store::{AccountPrice, Db, GameFilters, ImportRequest, MissingScope};
use chrono::Utc;

async fn setup() -> Db {
    let db = Db::memory().await.expect("内存库应能创建");
    db.migrate().await.expect("迁移应成功");
    db
}

fn profile(name: &str, country: &str) -> AccountProfile {
    AccountProfile {
        display_name: Some(name.to_string()),
        avatar_url: Some(format!("https://example.com/{name}.jpg")),
        profile_url: Some(format!("https://example.com/{name}")),
        country_code: Some(country.to_string()),
        metadata: Some(serde_json::json!({ "personaname": name, "loccountrycode": country })),
    }
}

fn wishlist_item(appid: &str, title: &str) -> SourceItem {
    SourceItem {
        source: Source::Steam,
        external_id: appid.to_string(),
        title: title.to_string(),
        status: None,
        ..Default::default()
    }
}

fn library_item(appid: &str, title: &str, playtime_minutes: i64) -> SourceItem {
    SourceItem {
        source: Source::Steam,
        external_id: appid.to_string(),
        title: title.to_string(),
        status: Some(if playtime_minutes > 0 {
            Status::Playing
        } else {
            Status::Backlog
        }),
        playtime_minutes: Some(playtime_minutes),
        playtime_2weeks_minutes: Some(0),
        ..Default::default()
    }
}

async fn import_for(
    db: &Db,
    account_id: i64,
    scope: MissingScope,
    items: &[SourceItem],
) -> acgn_core::ImportSummary {
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
        account_id: Some(account_id),
    })
    .await
    .expect("带账号的导入不应失败")
}

async fn dry_import_for(
    db: &Db,
    account_id: i64,
    scope: MissingScope,
    items: &[SourceItem],
) -> acgn_core::ImportSummary {
    db.import_items(&ImportRequest {
        source: Source::Steam,
        mode: match scope {
            MissingScope::Wishlist => "steam_wishlist",
            MissingScope::Library => "steam_library",
        },
        items,
        options: ImportOptions {
            dry_run: true,
            ..Default::default()
        },
        args: None,
        source_mode: "test",
        extra_warnings: Vec::new(),
        missing_scope: scope,
        account_id: Some(account_id),
    })
    .await
    .expect("带账号的预览不应失败")
}

async fn account_of(db: &Db, external_id: &str) -> AccountSummary {
    db.list_accounts()
        .await
        .unwrap()
        .into_iter()
        .find(|summary| summary.account.external_id == external_id)
        .unwrap_or_else(|| panic!("账号 {external_id} 应存在"))
}

async fn get(db: &Db, appid: &str) -> acgn_core::GameWithEntry {
    let mut item = db
        .get_game_by_external(Source::Steam, appid)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("条目 {appid} 应存在"));
    db.attach_accounts(std::slice::from_mut(&mut item))
        .await
        .unwrap();
    item
}

#[tokio::test]
async fn upsert_account_is_idempotent_and_profile_updates_are_merged() {
    let db = setup().await;

    let (account, created) = db
        .upsert_account(
            Source::Steam,
            "76561198000000001",
            Some(&profile("Main", "cn")),
        )
        .await
        .unwrap();
    assert!(created, "首次应新建");
    assert_eq!(account.display_name.as_deref(), Some("Main"));
    assert_eq!(account.effective_country.as_deref(), Some("cn"));
    let first_updated_at = account.updated_at;

    // 同一份资料重复写入：不新建、不刷新 updated_at
    let (again, created) = db
        .upsert_account(
            Source::Steam,
            "76561198000000001",
            Some(&profile("Main", "cn")),
        )
        .await
        .unwrap();
    assert!(!created, "重复解析不应新建账号");
    assert_eq!(again.id, account.id);
    assert_eq!(
        again.updated_at, first_updated_at,
        "无变化时不应改 updated_at"
    );

    // 昵称变化：覆盖
    let (renamed, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000001",
            Some(&profile("Renamed", "cn")),
        )
        .await
        .unwrap();
    assert_eq!(renamed.display_name.as_deref(), Some("Renamed"));

    // 资料抓取失败（profile = None）：保留已有字段
    let (kept, created) = db
        .upsert_account(Source::Steam, "76561198000000001", None)
        .await
        .unwrap();
    assert!(!created);
    assert_eq!(kept.display_name.as_deref(), Some("Renamed"));

    // 手动覆盖区域：effective_country 优先取 override；清空后回到资料值
    let overridden = db
        .set_account_country_override(account.id, Some("us"))
        .await
        .unwrap();
    assert_eq!(overridden.country_override.as_deref(), Some("us"));
    assert_eq!(overridden.effective_country.as_deref(), Some("us"));

    let cleared = db
        .set_account_country_override(account.id, None)
        .await
        .unwrap();
    assert!(cleared.country_override.is_none());
    assert_eq!(cleared.effective_country.as_deref(), Some("cn"));

    let list = db.list_accounts().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].owned_count, 0);
    assert_eq!(list[0].wishlist_count, 0);
}

#[tokio::test]
async fn library_import_links_games_and_aggregates_playtime_across_accounts() {
    let db = setup().await;
    let (main, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000001",
            Some(&profile("Main", "cn")),
        )
        .await
        .unwrap();
    let (alt, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000002",
            Some(&profile("Alt", "us")),
        )
        .await
        .unwrap();

    // 主号库存：玩过 120 分钟 + 买了没玩
    let summary = import_for(
        &db,
        main.id,
        MissingScope::Library,
        &[
            library_item("1", "Played", 120),
            library_item("2", "Not Played", 0),
        ],
    )
    .await;
    assert_eq!(
        (summary.added, summary.updated, summary.unchanged),
        (2, 0, 0)
    );

    let played = get(&db, "1").await;
    assert_eq!(played.entry.playtime_minutes, Some(120));
    assert_eq!(played.entry.source_playtime_minutes, Some(120));
    assert_eq!(played.entry.status, Status::Playing);
    assert_eq!(played.accounts.len(), 1, "条目应带出账号关联");
    assert_eq!(played.accounts[0].account_id, main.id);
    assert!(played.accounts[0].owned);
    assert_eq!(played.accounts[0].source_playtime_minutes, Some(120));

    // 小号也拥有同一个游戏、只玩了 30 分钟：有效时长保持主号的 120（MAX 聚合）
    import_for(
        &db,
        alt.id,
        MissingScope::Library,
        &[library_item("1", "Played", 30)],
    )
    .await;
    let played = get(&db, "1").await;
    assert_eq!(
        played.entry.source_playtime_minutes,
        Some(120),
        "小号同步不能把主号时长覆盖低"
    );
    assert_eq!(played.entry.playtime_minutes, Some(120));
    assert_eq!(played.accounts.len(), 2);
    assert_eq!(
        played.accounts[1].source_playtime_minutes,
        Some(30),
        "每个账号保存自己的时长"
    );

    // 重复同步完全相同的库存：no-op
    let again = import_for(
        &db,
        main.id,
        MissingScope::Library,
        &[
            library_item("1", "Played", 120),
            library_item("2", "Not Played", 0),
        ],
    )
    .await;
    assert_eq!(
        (again.added, again.updated, again.unchanged),
        (0, 0, 2),
        "重复导入必须幂等"
    );

    // 预览与真实导入同一口径：数据没变 → unchanged
    let preview = dry_import_for(
        &db,
        main.id,
        MissingScope::Library,
        &[library_item("1", "Played", 120)],
    )
    .await;
    assert_eq!((preview.updated, preview.unchanged), (0, 1));

    // 用户手填时长仍然优先；聚合值只更新 source_playtime_minutes
    db.update_entry(
        played.game.id,
        &EntryPatch {
            playtime_minutes: Some(500),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    import_for(
        &db,
        main.id,
        MissingScope::Library,
        &[library_item("1", "Played", 150)],
    )
    .await;
    let entry = get(&db, "1").await.entry;
    assert_eq!(entry.playtime_minutes, Some(500), "本地手填优先");
    assert_eq!(entry.source_playtime_minutes, Some(150));

    // 账号统计
    let main_summary = account_of(&db, "76561198000000001").await;
    assert_eq!(main_summary.owned_count, 2);
    assert_eq!(main_summary.wishlist_count, 0);
    assert!(main_summary.account.last_synced_at.is_some());
    let alt_summary = account_of(&db, "76561198000000002").await;
    assert_eq!(alt_summary.owned_count, 1);
}

#[tokio::test]
async fn wishlist_and_missing_are_scoped_per_account() {
    let db = setup().await;
    let (a, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000001",
            Some(&profile("A", "cn")),
        )
        .await
        .unwrap();
    let (b, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000002",
            Some(&profile("B", "us")),
        )
        .await
        .unwrap();

    import_for(
        &db,
        a.id,
        MissingScope::Wishlist,
        &[
            wishlist_item("10", "A Wants"),
            wishlist_item("11", "Both Want"),
        ],
    )
    .await;
    import_for(
        &db,
        b.id,
        MissingScope::Wishlist,
        &[wishlist_item("11", "Both Want")],
    )
    .await;

    assert_eq!(account_of(&db, "76561198000000001").await.wishlist_count, 2);
    assert_eq!(account_of(&db, "76561198000000002").await.wishlist_count, 1);

    // 按账号筛选：A 的愿望单 2 条，B 的 1 条
    let page = db
        .list_games(
            &GameFilters::from_query(&GameListQuery {
                account_id: Some(a.id),
                ..Default::default()
            }),
            None,
            1,
            10,
        )
        .await
        .unwrap();
    assert_eq!(page.total, 2);
    let page = db
        .list_games(
            &GameFilters::from_query(&GameListQuery {
                account_id: Some(b.id),
                ..Default::default()
            }),
            None,
            1,
            10,
        )
        .await
        .unwrap();
    assert_eq!(page.total, 1);

    // A 的愿望单少了 11：只报 A 的 missing，B 不受影响
    let summary = import_for(
        &db,
        a.id,
        MissingScope::Wishlist,
        &[wishlist_item("10", "A Wants")],
    )
    .await;
    assert_eq!(summary.missing, 1, "A 的 missing 口径只统计 A 的愿望单");

    let summary = import_for(
        &db,
        b.id,
        MissingScope::Wishlist,
        &[wishlist_item("11", "Both Want")],
    )
    .await;
    assert_eq!(summary.missing, 0, "B 的愿望单没有变化");

    // 库存同步买下 11：该账号的愿望单标记应清除（Steam 买到手会移出愿望单）
    import_for(
        &db,
        a.id,
        MissingScope::Library,
        &[library_item("11", "Both Want", 5)],
    )
    .await;
    assert_eq!(account_of(&db, "76561198000000001").await.wishlist_count, 1);
    let entry = get(&db, "11").await;
    let link = entry
        .accounts
        .iter()
        .find(|link| link.account_id == a.id)
        .expect("A 的关联应存在");
    assert!(link.owned);
    assert!(!link.in_wishlist, "买到手后不再是愿望单");
    assert_eq!(link.source_playtime_minutes, Some(5));

    // B 的愿望单标记不受 A 的库存同步影响
    assert_eq!(account_of(&db, "76561198000000002").await.wishlist_count, 1);
}

#[tokio::test]
async fn library_missing_is_scoped_per_account() {
    let db = setup().await;
    let (a, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000001",
            Some(&profile("A", "cn")),
        )
        .await
        .unwrap();
    let (b, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000002",
            Some(&profile("B", "us")),
        )
        .await
        .unwrap();

    import_for(
        &db,
        a.id,
        MissingScope::Library,
        &[library_item("20", "Owned By Both", 10)],
    )
    .await;
    import_for(
        &db,
        b.id,
        MissingScope::Library,
        &[library_item("20", "Owned By Both", 99)],
    )
    .await;

    // A 退款（库存里没了）：只报 A 的 1 条；游戏仍然保留
    let summary = import_for(&db, a.id, MissingScope::Library, &[]).await;
    assert_eq!(summary.missing, 1);
    assert!(
        summary
            .warnings
            .iter()
            .any(|warning| warning.contains("退款"))
    );
    assert_eq!(get(&db, "20").await.game.title, "Owned By Both");

    // B 的库存没有变化：missing 为 0
    let summary = import_for(
        &db,
        b.id,
        MissingScope::Library,
        &[library_item("20", "Owned By Both", 99)],
    )
    .await;
    assert_eq!(summary.missing, 0);
}

#[tokio::test]
async fn account_filter_attaches_links_and_delete_keeps_games() {
    let db = setup().await;
    let (a, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000001",
            Some(&profile("A", "cn")),
        )
        .await
        .unwrap();
    let (b, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000002",
            Some(&profile("B", "us")),
        )
        .await
        .unwrap();

    import_for(
        &db,
        a.id,
        MissingScope::Library,
        &[library_item("30", "Only A", 60)],
    )
    .await;
    import_for(
        &db,
        a.id,
        MissingScope::Wishlist,
        &[wishlist_item("31", "A Wishlist")],
    )
    .await;
    import_for(
        &db,
        b.id,
        MissingScope::Library,
        &[library_item("32", "Only B", 0)],
    )
    .await;

    // A 的筛选同时包含库存 + 愿望单
    let mut page = db
        .list_games(
            &GameFilters::from_query(&GameListQuery {
                account_id: Some(a.id),
                ..Default::default()
            }),
            Some("title"),
            1,
            10,
        )
        .await
        .unwrap();
    assert_eq!(page.total, 2);
    db.attach_accounts(&mut page.items).await.unwrap();
    assert_eq!(page.items[0].game.title, "A Wishlist");
    assert!(
        page.items[0].accounts[0].in_wishlist,
        "愿望单条目的关联应带出 in_wishlist"
    );
    assert_eq!(page.items[1].accounts.len(), 1);
    assert!(page.items[1].accounts[0].owned);

    // 删除账号：关联级联删除，游戏条目保留
    let game_id = get(&db, "30").await.game.id;
    assert!(db.delete_account(a.id).await.unwrap());
    assert!(
        !db.delete_account(a.id).await.unwrap(),
        "重复删除应返回 false"
    );
    assert_eq!(get(&db, "30").await.game.title, "Only A");
    assert!(db.accounts_for_game(game_id).await.unwrap().is_empty());
    assert_eq!(db.list_accounts().await.unwrap().len(), 1);
}

#[tokio::test]
async fn account_prices_are_regional_and_override_clears_cache() {
    let db = setup().await;
    let (a, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000001",
            Some(&profile("A", "cn")),
        )
        .await
        .unwrap();
    let (b, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000002",
            Some(&profile("B", "us")),
        )
        .await
        .unwrap();

    import_for(
        &db,
        a.id,
        MissingScope::Library,
        &[library_item("40", "Shared", 10)],
    )
    .await;
    import_for(
        &db,
        b.id,
        MissingScope::Library,
        &[library_item("40", "Shared", 20)],
    )
    .await;

    // 与全局配置区域（cn）相同的账号不需要单独抓价
    let pending = db
        .pending_account_prices(Source::Steam, "cn", 20)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1, "只有 us 账号需要按区域补价");
    assert_eq!(pending[0].account_id, b.id);
    assert_eq!(pending[0].external_id, "40");
    assert_eq!(pending[0].country, "us");

    let now = Utc::now();
    let game_id = get(&db, "40").await.game.id;
    let written = db
        .update_account_price(
            game_id,
            b.id,
            &AccountPrice {
                currency: Some("USD".to_string()),
                initial: Some(2999),
                final_price: Some(1499),
                discount_percent: Some(50),
                is_free: Some(false),
            },
            now,
        )
        .await
        .unwrap();
    assert!(written);

    assert!(
        db.pending_account_prices(Source::Steam, "cn", 20)
            .await
            .unwrap()
            .is_empty(),
        "已经抓过的区域不应重复排队"
    );

    let entry = get(&db, "40").await;
    let link = entry
        .accounts
        .iter()
        .find(|link| link.account_id == b.id)
        .unwrap();
    assert_eq!(link.price_currency.as_deref(), Some("USD"));
    assert_eq!(link.price_final, Some(1499));
    assert!(link.price_updated_at.is_some());
    let main_link = entry
        .accounts
        .iter()
        .find(|link| link.account_id == a.id)
        .unwrap();
    assert!(
        main_link.price_final.is_none() && main_link.price_updated_at.is_none(),
        "全局区域的账号不单独存价（展示时回退到全局价格）"
    );

    // 修改区域：清掉旧区域的价格缓存，并重新进入待补价队列
    db.set_account_country_override(b.id, Some("jp"))
        .await
        .unwrap();
    let entry = get(&db, "40").await;
    let link = entry
        .accounts
        .iter()
        .find(|link| link.account_id == b.id)
        .unwrap();
    assert!(link.price_final.is_none(), "区域变了，旧价格必须清掉");
    assert!(link.price_updated_at.is_none());
    let pending = db
        .pending_account_prices(Source::Steam, "cn", 20)
        .await
        .unwrap();
    assert!(pending.iter().any(|target| target.account_id == b.id));
}

#[tokio::test]
async fn dry_run_with_account_writes_nothing_but_reports_the_same_diff() {
    let db = setup().await;
    let (account, _) = db
        .upsert_account(
            Source::Steam,
            "76561198000000001",
            Some(&profile("A", "cn")),
        )
        .await
        .unwrap();

    let preview = dry_import_for(
        &db,
        account.id,
        MissingScope::Library,
        &[library_item("50", "New", 10)],
    )
    .await;
    assert_eq!(preview.added, 1);
    assert_eq!(preview.run_id, 0);
    assert_eq!(
        db.list_games(&GameFilters::default(), None, 1, 10)
            .await
            .unwrap()
            .total,
        0,
        "预览不写游戏"
    );
    assert_eq!(
        account_of(&db, "76561198000000001").await.owned_count,
        0,
        "预览不写账号关联"
    );
    assert!(
        account_of(&db, "76561198000000001")
            .await
            .account
            .last_synced_at
            .is_none(),
        "预览不刷新账号同步时间"
    );

    let real = import_for(
        &db,
        account.id,
        MissingScope::Library,
        &[library_item("50", "New", 10)],
    )
    .await;
    assert_eq!(real.added, 1);
    assert_eq!(account_of(&db, "76561198000000001").await.owned_count, 1);
}
