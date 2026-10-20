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
  /** 关联的数据源账号（库存 / 愿望单）；没有关联时字段缺省 */
  accounts?: GameAccount[];
}

/** 数据源账号（SteamID64）。不是本项目的多用户账号。 */
export interface Account {
  id: number;
  source: Source;
  external_id: string;
  display_name: string | null;
  avatar_url: string | null;
  profile_url: string | null;
  /** 资料里的区域（Steam loccountrycode） */
  country_code: string | null;
  /** 手动覆盖的价格区域；空 = 跟随资料 */
  country_override: string | null;
  /** 实际取价区域（override ?? 资料区域） */
  effective_country: string | null;
  /** 来源原始资料（GetPlayerSummaries 的 player 对象） */
  metadata: Record<string, unknown> | null;
  metadata_updated_at: string | null;
  last_synced_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface AccountSummary extends Account {
  /** 该账号库存条目数（含 0 分钟） */
  owned_count: number;
  /** 该账号愿望单条目数 */
  wishlist_count: number;
}

/** 游戏 ↔ 账号关联（随 GameWithEntry 返回）。 */
export interface GameAccount {
  account_id: number;
  source: Source;
  external_id: string;
  display_name: string | null;
  avatar_url: string | null;
  /** 该账号的实际取价区域 */
  country: string | null;
  in_wishlist: boolean;
  owned: boolean;
  source_playtime_minutes: number | null;
  playtime_2weeks_minutes: number | null;
  last_played_at: string | null;
  price_currency: string | null;
  price_initial: number | null;
  price_final: number | null;
  discount_percent: number | null;
  is_free: boolean | null;
  /** 非空 = 该账号区域已经抓过价（无价即「该区域没有售价」） */
  price_updated_at: string | null;
  last_synced_at: string | null;
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
  /** 本轮写入的账号区域价格条数 */
  account_prices: number;
  warnings: string[];
  source_mode: string;
  duration_ms: number;
}

/** 成就同步（`POST /api/imports/steam/achievements`）的结果 */
export interface AchievementSyncSummary {
  /** 本次拿到成就数据的条目数（该账号确实拥有且有统计） */
  checked: number;
  /** 写库发生了变化的条目数（含 promoted） */
  updated: number;
  /** 其中：因「全成就」被标记为已通关的条数 */
  promoted: number;
  /** 拿到数据但没有任何变化 */
  unchanged: number;
  /** 该账号没有这个游戏（未拥有 / 隐私）：只刷新了检查时间 */
  unavailable: number;
  /** 请求失败（网络 / 接口异常），再点一次会重试 */
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
  /** 只看某个账号关联的条目（库存 + 愿望单） */
  account_id?: number;
  q?: string;
  sort?: string;
  favorite?: boolean;
  page?: number;
  per_page?: number;
}

export interface ApiErrorBody {
  error?: { kind?: string; message?: string; hint?: string };
}
