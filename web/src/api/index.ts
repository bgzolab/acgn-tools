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
  preview?: boolean;
}

export const importJson = (body: JsonImportBody) =>
  http.post<ImportResponse>("/imports", body);

export const listRuns = (limit = 20) => http.get<ImportRun[]>(`/imports?limit=${limit}`);

export const enrich = (limit = 20) =>
  http.post<EnrichSummary>("/enrich", { limit });

// ------------------------------------------------------------------ 导出

export const exportUrl = `${API_BASE}/export/json`;
