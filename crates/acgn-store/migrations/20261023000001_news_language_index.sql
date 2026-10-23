-- 新闻列表的主查询是「按语言过滤 + 发布时间倒序」：
--   SELECT … FROM game_news WHERE language = ? ORDER BY published_at DESC, id DESC
-- 已有的 idx_game_news_published 不含 language，idx_game_news_game 又以 game_id 打头，
-- 都帮不上这个查询。补一个 (language, published_at DESC, id DESC) 组合索引。
CREATE INDEX IF NOT EXISTS idx_game_news_language_published
    ON game_news (language, published_at DESC, id DESC);
