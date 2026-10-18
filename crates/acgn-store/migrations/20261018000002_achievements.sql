-- 成就进度（Steam GetPlayerAchievements 的汇总）
--
-- 用途：
--   * achievements_achieved / achievements_total =「46/46」这种展示值，
--     也是「全成就 = 已通关」判定的依据（total > 0 且 achieved >= total）；
--   * achievements_checked_at = 最近一次检查时间，用于轮询：最久没查过的优先，
--     避免每次都对全库重复查一遍。
-- NULL 语义：achieved/total 为 NULL = 还从没拿到过成就数据（未拥有 / 隐私 / 无成就集）。
ALTER TABLE library_entries ADD COLUMN achievements_achieved INTEGER;
ALTER TABLE library_entries ADD COLUMN achievements_total INTEGER;
ALTER TABLE library_entries ADD COLUMN achievements_checked_at TEXT;
