/** 展示层格式化。 */

import { SOURCE_LABELS, STATUS_LABELS, type Game, type MediaKind, type Source, type Status } from "../api/types";
import { MEDIA_KIND_LABELS } from "../api/types";

export function statusLabel(status: Status): string {
  return STATUS_LABELS[status] ?? status;
}

export function sourceLabel(source: Source): string {
  return SOURCE_LABELS[source] ?? source;
}

export function mediaKindLabel(kind: MediaKind): string {
  return MEDIA_KIND_LABELS[kind] ?? kind;
}

/** 条目主标题：优先中文名。 */
export function displayTitle(game: Game): string {
  return game.title_zh?.trim() || game.title?.trim() || `#${game.external_id}`;
}

/** 副标题（英文原名），与主标题重复时返回空串。 */
export function secondaryTitle(game: Game): string {
  const zh = game.title_zh?.trim();
  const title = game.title?.trim() ?? "";
  if (!zh || !title || zh === title) return "";
  return title;
}

const CURRENCY_SYMBOL: Record<string, string> = {
  CNY: "¥",
  USD: "$",
  EUR: "€",
  JPY: "¥",
  GBP: "£",
  HKD: "HK$",
  TWD: "NT$",
};

/** 分 → 货币字符串。 */
export function formatPrice(cents: number | null | undefined, currency: string | null | undefined): string {
  if (cents === null || cents === undefined) return "—";
  if (cents === 0) return "免费";
  const code = currency ?? "CNY";
  const symbol = CURRENCY_SYMBOL[code] ?? `${code} `;
  const value = (cents / 100).toLocaleString("zh-CN", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  });
  return `${symbol}${value}`;
}

/** 价格段落：折扣时同时给出原价。 */
export function priceParts(game: Game): { final: string; initial: string | null; discount: number } {
  const discount = game.discount_percent ?? 0;
  const final = formatPrice(game.price_final ?? game.price_initial, game.price_currency);
  const initial =
    discount > 0 && game.price_initial
      ? formatPrice(game.price_initial, game.price_currency)
      : null;
  return { final, initial, discount };
}

export function formatMinutes(minutes: number | null | undefined): string {
  if (!minutes || minutes <= 0) return "—";
  const hours = minutes / 60;
  if (hours < 1) return `${minutes} 分钟`;
  return `${hours.toFixed(hours >= 10 ? 0 : 1)} 小时`;
}

export function formatNumber(value: number | null | undefined): string {
  if (value === null || value === undefined) return "—";
  return value.toLocaleString("zh-CN");
}

export function formatDate(value: string | null | undefined): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleDateString("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit" });
}

export function formatDateTime(value: string | null | undefined): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function relativeTime(value: string | null | undefined): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const diff = Date.now() - date.getTime();
  const minutes = Math.round(diff / 60000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  const days = Math.round(hours / 24);
  if (days < 30) return `${days} 天前`;
  const months = Math.round(days / 30);
  if (months < 12) return `${months} 个月前`;
  return `${Math.round(months / 12)} 年前`;
}

/** 好评档位 → 颜色（0..9）。 */
export function reviewColor(score: number | null | undefined): string {
  if (score === null || score === undefined) return "var(--muted)";
  if (score >= 8) return "var(--ok)";
  if (score >= 6) return "#39c5cf";
  if (score >= 4) return "var(--warn)";
  return "var(--danger)";
}

export function reviewLabel(game: Game): string {
  if (!game.review_desc) return "—";
  return game.review_desc;
}

/** 简单的中文好评档位翻译，避免界面全英文。 */
const REVIEW_ZH: Record<string, string> = {
  "Overwhelmingly Positive": "好评如潮",
  "Very Positive": "特别好评",
  Positive: "好评",
  "Mostly Positive": "多半好评",
  Mixed: "褒贬不一",
  "Mostly Negative": "多半差评",
  Negative: "差评",
  "Very Negative": "特别差评",
  "Overwhelmingly Negative": "差评如潮",
};

export function reviewLabelZh(game: Game): string {
  if (!game.review_desc) return "暂无评价";
  return REVIEW_ZH[game.review_desc] ?? game.review_desc;
}
