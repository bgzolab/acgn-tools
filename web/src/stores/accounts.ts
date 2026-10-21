/** 数据源账号列表（导入页 / 库筛选共用；后端是唯一真相）。 */

import { defineStore } from "pinia";
import { ref } from "vue";

import { listAccounts } from "../api";
import type { AccountSummary } from "../api/types";

export const useAccountsStore = defineStore("accounts", () => {
  const items = ref<AccountSummary[]>([]);
  const loading = ref(false);
  const error = ref("");

  /** 进行中的请求：并发调用复用同一个 Promise，`await` 之后一定是最新列表。 */
  let inflight: Promise<void> | null = null;

  /**
   * 每次都重新拉取：账号是本地 SQLite 的一张小表，请求很便宜；
   * 页面挂载 / 同步结束 / 删除账号后都希望看到最新数据。
   */
  function load(): Promise<void> {
    if (inflight) return inflight;

    inflight = (async () => {
      loading.value = true;
      error.value = "";
      try {
        items.value = await listAccounts();
      } catch (err) {
        error.value = err instanceof Error ? err.message : String(err);
      } finally {
        loading.value = false;
        inflight = null;
      }
    })();

    return inflight;
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

  return { items, loading, error, load, byId, label };
});
