/** 与 Rust 侧 `acgn-core` 的模型一一对应（JSON 都是 snake_case）。 */

export type Status =
  | "wishlist"
  | "backlog"
  | "playing"
  | "completed"
  | "on_hold"
  | "dropped"
  | "ignored";

export const STATUS_ORDER: Status[] = [
  "wishlist",
  "backlog",
  "playing",
  "completed",
  "on_hold",
  "dropped",
  "ignored",
];

export const STATUS_LABELS: Record<Status, string> = {
  wishlist: "愿望单",
  backlog: "待玩",
  playing: "在玩",
  completed: "已通关",
  on_hold: "搁置",
  dropped: "弃坑",
  ignored: "忽略",
};

export type Source = "steam" | "bangumi" | "xiaoheihe" | "manual";

export const SOURCE_LABELS: Record<Source, string> = {
  steam: "Steam",
  bangumi: "Bangumi",
  xiaoheihe: "小黑盒",
  manual: "手动",
};

export type MediaKind = "game" | "dlc" | "bundle" | "software" | "demo" | "other";

export const MEDIA_KIND_LABELS: Record<MediaKind, string> = {
  game: "游戏",
  dlc: "DLC",
  bundle: "捆绑包",
  software: "软件",
  demo: "试玩",
  other: "其他",
};

export interface Game {
  id: number;
  source: Source;
  external_id: string;
  media_kind: MediaKind;
  title: string;
  title_zh: string | null;
  short_description: string | null;
  cover_url: string | null;
  poster_url: string | null;
  release_date: string | null;
  release_raw: string | null;
  review_score: number | null;
  review_desc: string | null;
  review_count: number | null;
  is_free: boolean | null;
  price_currency: string | null;
  /** 单位：分 */
  price_initial: number | null;
  price_final: number | null;
  discount_percent: number | null;
  developers: string[];
  publishers: string[];
  genres: string[];
  platforms: string[];
  tags: string[];
  metadata_updated_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface LibraryEntry {
  game_id: number;
  status: Status;
  priority: number | null;
  favorite: boolean;
  rating: number | null;
  progress_percent: number | null;
  playtime_minutes: number | null;
  /** 最近一次来源导入报告的时长（Steam playtime_forever）；用于「手动优先」与展示 */
  source_playtime_minutes: number | null;
  /** 成就进度（achieved/total）；total = 0 表示该游戏没有成就集 */
  achievements_achieved: number | null;
  achievements_total: number | null;
  /** 最近一次检查成就的时间（轮询：最久没查的优先） */
  achievements_checked_at: string | null;
  note: string | null;
  source_added_at: string | null;
  started_at: string | null;
  finished_at: string | null;
  last_synced_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface GameWithEntry {
  game: Game;
  entry: LibraryEntry;
}

export interface Page<T> {
  items: T[];
  total: number;
  page: number;
  per_page: number;
  total_pages: number;
}

export interface StatusCount {
  status: Status;
  label: string;
  count: number;
}

export interface SourceCount {
  source: Source;
  label: string;
  count: number;
}

export interface MoneyBucket {
  currency: string;
  list_price_cents: number;
  current_price_cents: number;
  item_count: number;
}

export interface ImportRun {
  id: number;
  source: Source;
  mode: string;
  status: string;
  seen: number;
  added: number;
  updated: number;
  unchanged: number;
  skipped: number;
  missing: number;
  warnings: string[];
  error: string | null;
  args: Record<string, unknown> | null;
  started_at: string;
  finished_at: string | null;
  duration_ms: number | null;
}

export interface ImportSummary {
  run_id: number;
  source: Source;
  mode: string;
  seen: number;
  added: number;
  updated: number;
  unchanged: number;
  skipped: number;
  missing: number;
  warnings: string[];
  duration_ms: number;
  sample_ids: number[];
  source_mode: string;
}

export interface ImportResponse {
  summary: ImportSummary;
  /** 仅预览（dry_run）时为 null */
  run: ImportRun | null;
  preview: GameWithEntry[];
}

export interface Stats {
  total_games: number;
  by_status: StatusCount[];
  by_source: SourceCount[];
  favorite_count: number;
  total_playtime_minutes: number;
  average_rating: number | null;
  wishlist_value: MoneyBucket[];
  recent_imports: ImportRun[];
  recently_added: GameWithEntry[];
  playing: GameWithEntry[];
}

export interface EnrichSummary {
  requested: number;
  succeeded: number;
  failed: number;
  skipped: number;
  warnings: string[];
  source_mode: string;
  duration_ms: number;
}

/** 成就同步（`POST /api/imports/steam/achievements`）的结果 */
export interface AchievementSyncSummary {
  checked: number;
  /** 因「全成就」被标记为已通关的条数 */
  promoted: number;
  unchanged: number;
  /** 取不到成就数据（未拥有 / 隐私）的条数 */
  failed: number;
  /** 仍未检查过的条目数（再点一次继续） */
  pending: number;
  source_mode: string;
  duration_ms: number;
  warnings: string[];
}

export interface PublicConfig {
  app: string;
  version: string;
  schema_revision: number;
  steam: {
    mode: string;
    country: string;
    language: string;
    has_api_key: boolean;
    fixture: string | null;
  };
  statuses: { key: Status; label: string }[];
  default_per_page: number;
}

export interface EntryPatch {
  status?: Status;
  favorite?: boolean;
  rating?: number;
  progress_percent?: number;
  playtime_minutes?: number;
  note?: string;
  priority?: number;
}

export interface GameListParams {
  status?: Status | "";
  source?: Source;
  q?: string;
  sort?: string;
  favorite?: boolean;
  page?: number;
  per_page?: number;
}

export interface ApiErrorBody {
  error?: { kind?: string; message?: string; hint?: string };
}
