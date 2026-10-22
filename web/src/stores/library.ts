/** 游戏库列表状态（筛选 / 排序 / 分页 / 就地更新）。 */

import { defineStore } from "pinia";
import { reactive, ref } from "vue";

import { listGames, patchGame } from "../api";
import type { EntryPatch, GameWithEntry, Status } from "../api/types";

export interface LibraryFilters {
  status: Status | "";
  q: string;
  sort: string;
  favorite: boolean;
  /** 只看某个数据源账号关联的条目（null = 全部） */
  account_id: number | null;
  page: number;
  per_page: number;
}

export const SORT_OPTIONS = [
  { value: "added_desc", label: "最近加入" },
  { value: "added_asc", label: "最早加入" },
  { value: "updated_desc", label: "最近操作" },
  { value: "title", label: "名称 A→Z" },
  { value: "review_desc", label: "好评优先" },
  { value: "rating_desc", label: "我的评分" },
  { value: "priority", label: "愿望单排位" },
  { value: "price_desc", label: "价格从高到低" },
  { value: "playtime_desc", label: "时长" },
];

export const useLibraryStore = defineStore("library", () => {
  const items = ref<GameWithEntry[]>([]);
  const total = ref(0);
  const totalPages = ref(1);
  const loading = ref(false);
  const error = ref("");
  const lastLoadedAt = ref<number | null>(null);

  const filters = reactive<LibraryFilters>({
    status: "",
    q: "",
    sort: "added_desc",
    favorite: false,
    account_id: null,
    page: 1,
    per_page: 48,
  });

  async function load() {
    loading.value = true;
    error.value = "";
    try {
      const page = await listGames({
        status: filters.status || undefined,
        q: filters.q.trim() || undefined,
        sort: filters.sort,
        favorite: filters.favorite || undefined,
        account_id: filters.account_id ?? undefined,
        page: filters.page,
        per_page: filters.per_page,
      });
      items.value = page.items;
      total.value = page.total;
      totalPages.value = page.total_pages;
      lastLoadedAt.value = Date.now();
    } catch (err) {
      error.value = err instanceof Error ? err.message : String(err);
    } finally {
      loading.value = false;
    }
  }

  function refresh() {
    void load();
  }

  function setStatus(status: Status | "") {
    filters.status = status;
    filters.page = 1;
    refresh();
  }

  function setQuery(q: string) {
    filters.q = q;
    filters.page = 1;
    refresh();
  }

  function setSort(sort: string) {
    filters.sort = sort;
    filters.page = 1;
    refresh();
  }

  function toggleFavoriteOnly() {
    filters.favorite = !filters.favorite;
    filters.page = 1;
    refresh();
  }

  function setAccount(accountId: number | null) {
    filters.account_id = accountId;
    filters.page = 1;
    refresh();
  }

  function goPage(page: number) {
    filters.page = Math.min(Math.max(1, page), Math.max(1, totalPages.value));
    refresh();
  }

  /** 更新单条并就地替换（避免整页重载）。 */
  async function updateEntry(id: number, patch: EntryPatch) {
    const updated = await patchGame(id, patch);
    const index = items.value.findIndex((item) => item.game.id === id);
    if (index >= 0) {
      items.value[index] = updated;
    }
    return updated;
  }

  function replaceItem(updated: GameWithEntry) {
    const index = items.value.findIndex((item) => item.game.id === updated.game.id);
    if (index >= 0) items.value[index] = updated;
  }

  async function toggleFavorite(item: GameWithEntry) {
    return updateEntry(item.game.id, { favorite: !item.entry.favorite });
  }

  return {
    items,
    total,
    totalPages,
    loading,
    error,
    filters,
    lastLoadedAt,
    load,
    refresh,
    setStatus,
    setQuery,
    setSort,
    toggleFavoriteOnly,
    setAccount,
    goPage,
    updateEntry,
    replaceItem,
    toggleFavorite,
  };
});
