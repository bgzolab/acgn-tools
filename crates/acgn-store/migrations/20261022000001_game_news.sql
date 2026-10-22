-- 游戏新闻（Steam 公告 RSS）+ 每个游戏的新闻源同步状态
--
-- 背景：Steam 的公告 RSS 是「一个 appid 一个 feed」，没有批量接口。已拥有游戏可能
-- 有几百个，所以新闻落库、按需分批同步；列表只查本地库，不在请求路径里抓网络。
--
-- 约定：
--   * game_news 是**归档**：feed 只返回最近若干条，旧公告不会因此被删除；
--   * (game_id, language, guid) 唯一：同一份公告换语言会各存一份，展示层按语言过滤；
--   * summary 存纯文本（适配器剥离 HTML），前端直接渲染，不引入富文本 / XSS 面；
--   * news_feeds 记录每个游戏的语言级同步状态：last_fetched_at 每次尝试都推进，
--     last_success_at 只在成功时推进，last_error 保留最近一次失败原因；
--   * 失败的游戏仍保留已抓到的新闻，下次同步按「最久没抓的优先」轮到它。
CREATE TABLE IF NOT EXISTS game_news (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    game_id      INTEGER NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    language     TEXT    NOT NULL,
    guid         TEXT    NOT NULL,
    title        TEXT    NOT NULL,
    summary      TEXT,
    url          TEXT    NOT NULL,
    author       TEXT,
    published_at TEXT    NOT NULL,
    fetched_at   TEXT    NOT NULL,
    created_at   TEXT    NOT NULL,
    updated_at   TEXT    NOT NULL,
    UNIQUE (game_id, language, guid)
);

CREATE INDEX IF NOT EXISTS idx_game_news_published ON game_news (published_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_game_news_game ON game_news (game_id, language, published_at DESC);

CREATE TABLE IF NOT EXISTS news_feeds (
    game_id         INTEGER NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    language        TEXT    NOT NULL,
    feed_url        TEXT    NOT NULL,
    last_fetched_at TEXT    NOT NULL,
    last_success_at TEXT,
    last_error      TEXT,
    item_count      INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT    NOT NULL,
    updated_at      TEXT    NOT NULL,
    PRIMARY KEY (game_id, language)
);

CREATE INDEX IF NOT EXISTS idx_news_feeds_success ON news_feeds (language, last_success_at);
