-- 数据源账号（Steam 等）+ 游戏与账号的关联
--
-- 背景：一个人可能有多个 Steam 账号（主号 / 小号 / 外币区账号），每次同步都手输
-- steamID 既麻烦又容易错。账号是**数据源身份**，不是本项目的多用户体系。
--
-- 约定：
--   * (source, external_id) 唯一：同一个 SteamID64 只对应一条账号记录；
--   * 资料字段只存「方便展示的提取值」，完整载荷放 metadata_json（来源给多少存多少）；
--   * country_code 来自资料（loccountrycode），country_override 是手动覆盖的价格区：
--     实际取价区域 = COALESCE(country_override, country_code)。
--
-- game_accounts 是「游戏 ↔ 账号」的多对多关联（一个游戏可能被多个账号拥有 / 愿望单）：
--   * in_wishlist = 1：最近一次该账号的愿望单同步里还有它；
--   * source_playtime_minutes：该账号的累计时长；NULL = 该账号最近一次库存同步没有它
--     （0 是有效值：拥有但没玩过）。missing 的候选集按 (account, in_wishlist) /
--     (account, source_playtime_minutes IS NOT NULL) 分别取，互不串口径；
--   * price_*：按该账号区域抓到的价格（补全元数据时写入），price_updated_at 非空表示
--     「这个区域已经问过 Steam」，与「还没抓过」区分开——前者没价就展示「区域无售价」，
--     后者才回退到全局价格。
CREATE TABLE IF NOT EXISTS accounts (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    source              TEXT    NOT NULL,
    external_id         TEXT    NOT NULL,
    display_name        TEXT,
    avatar_url          TEXT,
    profile_url         TEXT,
    country_code        TEXT,
    country_override    TEXT,
    metadata_json       TEXT,
    metadata_updated_at TEXT,
    last_synced_at      TEXT,
    created_at          TEXT    NOT NULL,
    updated_at          TEXT    NOT NULL,
    UNIQUE (source, external_id)
);

CREATE INDEX IF NOT EXISTS idx_accounts_source ON accounts (source);

CREATE TABLE IF NOT EXISTS game_accounts (
    game_id                 INTEGER NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    account_id              INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    in_wishlist             INTEGER NOT NULL DEFAULT 0,
    source_playtime_minutes INTEGER,
    playtime_2weeks_minutes INTEGER,
    last_played_at          TEXT,
    price_currency          TEXT,
    price_initial           INTEGER,
    price_final             INTEGER,
    discount_percent        INTEGER,
    is_free                 INTEGER,
    price_updated_at        TEXT,
    last_synced_at          TEXT,
    created_at              TEXT    NOT NULL,
    updated_at              TEXT    NOT NULL,
    PRIMARY KEY (game_id, account_id)
);

CREATE INDEX IF NOT EXISTS idx_game_accounts_account ON game_accounts (account_id);
CREATE INDEX IF NOT EXISTS idx_game_accounts_prices ON game_accounts (account_id, price_updated_at);
