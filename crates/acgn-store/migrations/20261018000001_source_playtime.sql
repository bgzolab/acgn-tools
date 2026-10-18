-- 来源侧游玩时长（Steam GetOwnedGames 的 playtime_forever，单位：分钟）
--
-- 与 playtime_minutes 的关系（详见 docs/design.md §4.1）：
--   * source_playtime_minutes = 最近一次来源导入报告的值（来源跟踪，含 0）；
--   * playtime_minutes        = 展示 / 统计用的有效值，默认跟随来源，
--                               用户手动改过之后不再被导入覆盖（本地优先）。
-- 新增列可空：历史数据没有来源记录，全部视为「本地手填」。
ALTER TABLE library_entries ADD COLUMN source_playtime_minutes INTEGER;
