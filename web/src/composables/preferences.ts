/** 用户偏好（localStorage）：跨页面共享的轻量 UI 设置。 */

const ENRICH_LIMIT_KEY = "acgn.enrichLimit";
const DEFAULT_ENRICH_LIMIT = 20;
const MAX_ENRICH_LIMIT = 200;

/** 补全元数据每次的条数（导入页 / 游戏库 / 看板共用）。 */
export function readEnrichLimit(fallback = DEFAULT_ENRICH_LIMIT): number {
  const raw = Number(window.localStorage.getItem(ENRICH_LIMIT_KEY));
  if (!Number.isInteger(raw) || raw <= 0) return fallback;
  return Math.min(raw, MAX_ENRICH_LIMIT);
}

/** 保存并返回归一化后的条数（非法值回退默认，超过上限截断）。 */
export function writeEnrichLimit(value: number): number {
  const limit =
    Number.isInteger(value) && value > 0
      ? Math.min(value, MAX_ENRICH_LIMIT)
      : DEFAULT_ENRICH_LIMIT;
  window.localStorage.setItem(ENRICH_LIMIT_KEY, String(limit));
  return limit;
}
