/** 所有后端接口的类型化封装。 */

import { API_BASE, http } from "./http";
import type {
  Account,
  AccountSummary,
  AchievementSyncSummary,
  EnrichSummary,
  EntryPatch,
  Game,
  GameAccount,
  GameListParams,
  GameNews,
  GameWithEntry,
  ImportResponse,
  ImportRun,
  LibraryEntry,
  NewsListParams,
  NewsStatus,
  NewsSyncSummary,
  Page,
  PublicConfig,
  Stats,
} from "./types";

// ------------------------------------------------------------------ 元信息

export const getConfig = () => http.get<PublicConfig>("/config");

export const getHealth = () =>
  http.get<{ status: string; version: string; uptime_seconds: number; steam_mode: string }>(
    "/health",
  );

export const getStats = () => http.get<Stats>("/stats");

// ------------------------------------------------------------------ 游戏新闻

export function listNews(params: NewsListParams = {}) {
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null || value === "") continue;
    query.set(key, String(value));
  }
  const suffix = query.toString();
  return http.get<Page<GameNews>>(`/news${suffix ? `?${suffix}` : ""}`);
}

export const getNewsStatus = () => http.get<NewsStatus>("/news/status");

/** 同步下一批已拥有游戏的新闻（默认 30 个；可只同步单个 game_id） */
export const syncNews = (body: { limit?: number; game_id?: number } = {}) =>
  http.post<NewsSyncSummary>("/news/sync", body);

// ------------------------------------------------------------------ 数据源账号

export const listAccounts = () => http.get<AccountSummary[]>("/accounts");

/** 添加账号：解析 steamID / 个性域名 / 主页链接，抓取昵称、头像、区域（幂等） */
export const resolveAccount = (input: string) =>
  http.post<{
    account: Account;
    created: boolean;
    resolved_via: string;
    warnings: string[];
  }>("/accounts/resolve", { input });

/** 修改价格区域（空串 = 跟随 Steam 资料）；会清掉该账号旧区域的价格缓存 */
export const patchAccountCountry = (id: number, countryCode: string) =>
  http.patch<Account>(`/accounts/${id}`, { country_code: countryCode });

/** 删除账号：关联级联删除，游戏条目不删 */
export const deleteAccount = (id: number) => http.delete<void>(`/accounts/${id}`);

// ------------------------------------------------------------------ 游戏

export function listGames(params: GameListParams = {}) {
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null || value === "" || value === false) continue;
    query.set(key, String(value));
  }
  const suffix = query.toString();
  return http.get<Page<GameWithEntry>>(`/games${suffix ? `?${suffix}` : ""}`);
}

export const getGame = (id: number) => http.get<GameWithEntry>(`/games/${id}`);

export const patchGame = (id: number, patch: EntryPatch) =>
  http.patch<GameWithEntry>(`/games/${id}`, patch);

export const refreshMetadata = (id: number) =>
  http.post<{ outcome: string; game: Game; entry: LibraryEntry; accounts: GameAccount[] }>(
    `/games/${id}/refresh-metadata`,
  );

/** 删除单条（本地动作；来源里还在的下次导入会重新加回来） */
export const deleteGame = (id: number) => http.delete<void>(`/games/${id}`);

/** 批量删除（多选清理），返回实际删除行数（未知 id 忽略） */
export const deleteGames = (ids: number[]) =>
  http.post<{ deleted: number }>("/games/delete", { ids });

// ------------------------------------------------------------------ 导入 / 同步

export interface SteamImportBody {
  /** 账号 id（从账号列表选择；推荐） */
  account_id?: number;
  /** 兼容入口：17 位 steamID64 / 个性域名 / 主页链接（自动建号） */
  steam_id?: string;
  preview?: boolean;
  options?: { include_dlc?: boolean; include_non_games?: boolean; dry_run?: boolean };
}

export const importSteamWishlist = (body: SteamImportBody) =>
  http.post<ImportResponse>("/imports/steam/wishlist", body);

/** 导入 Steam 库存（已拥有游戏 + 游玩时长）；需要后端配置 STEAM_API_KEY */
export const importSteamLibrary = (body: SteamImportBody) =>
  http.post<ImportResponse>("/imports/steam/library", body);

/** 检查成就进度：全成就的游戏标记为「已通关」（逐游戏查询，可分批） */
export const syncSteamAchievements = (body: {
  account_id?: number;
  steam_id?: string;
  limit?: number;
}) => http.post<AchievementSyncSummary>("/imports/steam/achievements", body);

export interface JsonImportBody {
  items: unknown[];
  source?: string;
  mode?: string;
  /** 是否返回受影响的条目（与「是否写入」无关） */
  preview?: boolean;
  /** 只预览不落库用 `options.dry_run`（扁平导入）/ `dry_run`（还原） */
  options?: { dry_run?: boolean; include_dlc?: boolean; include_non_games?: boolean };
}

export const importJson = (body: JsonImportBody) =>
  http.post<ImportResponse>("/imports", body);

/** 备份还原：把 `GET /api/export/json` 的产物原样灌回来（文件优先） */
export interface RestoreBody {
  items: unknown[];
  dry_run?: boolean;
  preview?: boolean;
}

export const restoreJson = (body: RestoreBody) =>
  http.post<ImportResponse>("/imports/restore", body);

export const listRuns = (limit = 20) => http.get<ImportRun[]>(`/imports?limit=${limit}`);

export const enrich = (limit = 20) =>
  http.post<EnrichSummary>("/enrich", { limit });

// ------------------------------------------------------------------ 导出

export const exportUrl = `${API_BASE}/export/json`;
