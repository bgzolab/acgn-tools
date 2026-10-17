/** 所有后端接口的类型化封装。 */

import { API_BASE, http } from "./http";
import type {
  EnrichSummary,
  EntryPatch,
  Game,
  GameListParams,
  GameWithEntry,
  ImportResponse,
  ImportRun,
  LibraryEntry,
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
  http.post<{ outcome: string; game: Game; entry: LibraryEntry }>(
    `/games/${id}/refresh-metadata`,
  );

/** 删除单条（本地动作；来源里还在的下次导入会重新加回来） */
export const deleteGame = (id: number) => http.delete<void>(`/games/${id}`);

/** 批量删除（多选清理），返回实际删除行数（未知 id 忽略） */
export const deleteGames = (ids: number[]) =>
  http.post<{ deleted: number }>("/games/delete", { ids });

// ------------------------------------------------------------------ 导入 / 同步

export interface SteamImportBody {
  steam_id: string;
  preview?: boolean;
  options?: { include_dlc?: boolean; include_non_games?: boolean; dry_run?: boolean };
}

export const importSteamWishlist = (body: SteamImportBody) =>
  http.post<ImportResponse>("/imports/steam/wishlist", body);

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
