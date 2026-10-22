-- 新闻列表现在都按 language 过滤；`(language, published_at DESC, id DESC)` 已覆盖
-- 无 game_id 的查询，`(game_id, language, published_at DESC)` 覆盖单游戏查询。
-- 旧的 `(published_at DESC, id DESC)` 不再被任何查询选中，只会在每次写入时维护，
-- 因此在追加迁移里删掉。
DROP INDEX IF EXISTS idx_game_news_published;
