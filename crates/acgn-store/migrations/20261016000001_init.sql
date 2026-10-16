-- ACGN 管理器初始 schema（v1）
--
-- 约定：
--   * 所有时间戳以 RFC3339 TEXT 存储（sqlx 的 DateTime<Utc> 编解码格式一致，可字典序比较）
--   * 数组字段以 JSON 字符串存储，读出来再 parse
--   * games = 世界里的条目；library_entries = 我和条目的关系

CREATE TABLE IF NOT EXISTS games (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    source              TEXT    NOT NULL,
    external_id         TEXT    NOT NULL,
    media_kind          TEXT    NOT NULL DEFAULT 'game',
    title               TEXT    NOT NULL,
    title_zh            TEXT,
    short_description   TEXT,
    cover_url           TEXT,
    poster_url          TEXT,
    release_date        TEXT,
    release_raw         TEXT,
    review_score        INTEGER,
    review_desc         TEXT,
    review_count        INTEGER,
    is_free             INTEGER,
    price_currency      TEXT,
    price_initial       INTEGER,
    price_final         INTEGER,
    discount_percent    INTEGER,
    developers          TEXT    NOT NULL DEFAULT '[]',
    publishers          TEXT    NOT NULL DEFAULT '[]',
    genres              TEXT    NOT NULL DEFAULT '[]',
    platforms           TEXT    NOT NULL DEFAULT '[]',
    tags                TEXT    NOT NULL DEFAULT '[]',
    metadata_json       TEXT,
    metadata_updated_at TEXT,
    created_at          TEXT    NOT NULL,
    updated_at          TEXT    NOT NULL,
    UNIQUE (source, external_id)
);

CREATE TABLE IF NOT EXISTS library_entries (
    game_id             INTEGER PRIMARY KEY REFERENCES games (id) ON DELETE CASCADE,
    status              TEXT    NOT NULL DEFAULT 'wishlist',
    priority            INTEGER,
    favorite            INTEGER NOT NULL DEFAULT 0,
    rating              INTEGER,
    progress_percent    INTEGER,
    playtime_minutes    INTEGER,
    note                TEXT,
    source_added_at     TEXT,
    started_at          TEXT,
    finished_at         TEXT,
    last_synced_at      TEXT,
    created_at          TEXT    NOT NULL,
    updated_at          TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_library_entries_status ON library_entries (status);
CREATE INDEX IF NOT EXISTS idx_games_title ON games (title);
CREATE INDEX IF NOT EXISTS idx_games_metadata ON games (metadata_updated_at);

CREATE TABLE IF NOT EXISTS import_runs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    source        TEXT    NOT NULL,
    mode          TEXT    NOT NULL,
    status        TEXT    NOT NULL DEFAULT 'running',
    seen          INTEGER NOT NULL DEFAULT 0,
    added         INTEGER NOT NULL DEFAULT 0,
    updated       INTEGER NOT NULL DEFAULT 0,
    unchanged     INTEGER NOT NULL DEFAULT 0,
    skipped       INTEGER NOT NULL DEFAULT 0,
    missing       INTEGER NOT NULL DEFAULT 0,
    warnings      TEXT    NOT NULL DEFAULT '[]',
    error         TEXT,
    args          TEXT,
    started_at    TEXT    NOT NULL,
    finished_at   TEXT,
    duration_ms   INTEGER
);

CREATE INDEX IF NOT EXISTS idx_import_runs_started ON import_runs (started_at DESC);
