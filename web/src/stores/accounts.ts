/** 数据源账号列表（导入页 / 库筛选共用；后端是唯一真相）。 */

import { defineStore } from "pinia";
import { ref } from "vue";

import { listAccounts } from "../api";
import type { AccountSummary } from "../api/types";

/** 列表缓存时长：切页面不重复请求，超过这个时间后自动重新拉取一次 */
const CACHE_TTL_MS = 30_000;

export const useAccountsStore = defineStore("accounts", () => {
  const items = ref<AccountSummary[]>([]);
  const loading = ref(false);
  const error = ref("");
  const loadedAt = ref<number | null>(null);

  async function load(force = false) {
    if (loading.value) return;
    const fresh = loadedAt.value !== null && Date.now() - loadedAt.value < CACHE_TTL_MS;
    if (items.value.length && !force && fresh) return;
    loading.value = true;
    error.value = "";
    try {
      items.value = await listAccounts();
      loadedAt.value = Date.now();
    } catch (err) {
      error.value = err instanceof Error ? err.message : String(err);
    } finally {
      loading.value = false;
    }
  }

  function byId(id: number | null | undefined): AccountSummary | null {
    if (id === null || id === undefined) return null;
    return items.value.find((account) => account.id === id) ?? null;
  }

  /** 账号展示名兜底（与后端 `display_label` 一致）。 */
  function label(account: AccountSummary | null | undefined): string {
    if (!account) return "";
    const name = account.display_name?.trim();
    if (name) return name;
    const source = account.source === "steam" ? "Steam" : account.source;
    return `${source} ${account.external_id}`;
  }

  return { items, loading, error, loadedAt, load, byId, label };
});
