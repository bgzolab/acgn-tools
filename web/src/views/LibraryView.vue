<script setup lang="ts">
/** 游戏库：筛选 / 搜索 / 排序 / 分页 + 详情抽屉。 */
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";

import { deleteGames, enrich } from "../api";
import type { Status } from "../api/types";
import GameCard from "../components/GameCard.vue";
import GameDetailDrawer from "../components/GameDetailDrawer.vue";
import Pagination from "../components/Pagination.vue";
import { errorText, pushToast } from "../composables/toast";
import { SORT_OPTIONS, useLibraryStore } from "../stores/library";
import { useMetaStore } from "../stores/meta";

const store = useLibraryStore();
const meta = useMetaStore();
const route = useRoute();
const router = useRouter();

const searchDraft = ref(store.filters.q);
const openId = ref<number | null>(null);
const enriching = ref(false);

/** 批量管理：点卡片选中/取消，「删除选中」走二段确认 */
const managing = ref(false);
const selectedIds = ref<Set<number>>(new Set());
const deleting = ref(false);
const deleteArmed = ref(false);

const selectedCount = computed(() => selectedIds.value.size);
const allSelected = computed(
  () => store.items.length > 0 && store.items.every((item) => selectedIds.value.has(item.game.id)),
);

function toggleManaging() {
  managing.value = !managing.value;
  selectedIds.value = new Set();
  deleteArmed.value = false;
}

function toggleSelect(id: number) {
  const next = new Set(selectedIds.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  selectedIds.value = next;
  // 选择变化后重新武装确认按钮，避免误触「确认删除」
  deleteArmed.value = false;
}

function toggleSelectAll() {
  selectedIds.value = allSelected.value
    ? new Set()
    : new Set(store.items.map((item) => item.game.id));
  deleteArmed.value = false;
}

async function deleteSelected() {
  if (!deleteArmed.value) {
    deleteArmed.value = true;
    return;
  }
  const ids = [...selectedIds.value];
  if (!ids.length) return;

  deleting.value = true;
  try {
    const { deleted } = await deleteGames(ids);
    pushToast("ok", "已删除", `本地移除了 ${deleted} 条；来源里还在的下次导入会重新加回来`);
    selectedIds.value = new Set();
    await store.load();
    // 删空当前页时回退一页
    if (store.filters.page > Math.max(1, store.totalPages)) store.goPage(store.totalPages);
  } catch (error) {
    const { title, message } = errorText(error);
    pushToast("err", title, message);
  } finally {
    deleting.value = false;
    // 无论成败都回到未武装态：每次确认都必须重新经过两段确认
    deleteArmed.value = false;
  }
}

/** 详情抽屉里删掉单条后：关抽屉、刷列表、必要时回退一页 */
async function onDeleted() {
  closeDetail();
  await store.load();
  if (store.filters.page > Math.max(1, store.totalPages)) store.goPage(store.totalPages);
}

// 筛选变化后旧的选择不再可见：清空并解除武装，避免「确认删除 N 条」还亮着时误删看不见的条目
watch(
  () => [store.filters.status, store.filters.q, store.filters.sort, store.filters.favorite],
  () => {
    selectedIds.value = new Set();
    deleteArmed.value = false;
  },
);

// 翻页保留累计选择（跨页多选是特性），但确认按钮必须重新武装
watch(
  () => store.filters.page,
  () => {
    deleteArmed.value = false;
  },
);

const statuses = computed(() => meta.statuses);

let searchTimer: number | undefined;
watch(searchDraft, (value) => {
  window.clearTimeout(searchTimer);
  searchTimer = window.setTimeout(() => {
    if (value !== store.filters.q) store.setQuery(value);
  }, 320);
});

async function openDetail(id: number) {
  openId.value = id;
  if (route.query.focus !== String(id)) {
    void router.replace({ query: { ...route.query, focus: String(id) } });
  }
}

function closeDetail() {
  openId.value = null;
  if (route.query.focus) {
    const nextQuery = { ...route.query };
    delete nextQuery.focus;
    void router.replace({ query: nextQuery });
  }
}

async function runEnrich() {
  enriching.value = true;
  try {
    const result = await enrich(20);
    pushToast(
      result.succeeded > 0 ? "ok" : "warn",
      "元数据补全完成",
      `成功 ${result.succeeded} / 失败 ${result.failed}`,
    );
    store.refresh();
  } catch (error) {
    const { title, message } = errorText(error);
    pushToast("err", title, message);
  } finally {
    enriching.value = false;
  }
}

onMounted(async () => {
  await meta.load();
  const focus = Number(route.query.focus);
  if (Number.isFinite(focus) && focus > 0) openId.value = focus;
  void store.load();
});
</script>

<template>
  <div class="page-head">
    <div>
      <h1>游戏库</h1>
      <p class="sub">
        共 {{ store.total }} 条<template v-if="store.filters.q"> · 搜索「{{ store.filters.q }}」</template>
      </p>
    </div>
    <div class="row">
      <input
        v-model="searchDraft"
        class="input"
        style="width: 240px"
        type="search"
        placeholder="搜索标题 / 开发商 / 标签"
      />
      <select
        class="select"
        style="width: 132px"
        :value="store.filters.sort"
        @change="store.setSort(($event.target as HTMLSelectElement).value)"
      >
        <option v-for="option in SORT_OPTIONS" :key="option.value" :value="option.value">
          {{ option.label }}
        </option>
      </select>
      <button class="btn" :disabled="enriching" @click="runEnrich">
        {{ enriching ? "补全中…" : "补全元数据" }}
      </button>
      <button class="btn" :class="{ primary: managing }" @click="toggleManaging">
        {{ managing ? "退出管理" : "管理" }}
      </button>
    </div>
  </div>

  <div class="row" style="margin-bottom: 16px">
    <button class="chip" :class="{ active: store.filters.status === '' }" @click="store.setStatus('')">
      全部 <span class="count">{{ store.total }}</span>
    </button>
    <button
      v-for="status in statuses"
      :key="status.key"
      class="chip"
      :class="{ active: store.filters.status === status.key }"
      @click="store.setStatus(status.key as Status)"
    >
      {{ status.label }}
    </button>
    <button class="chip" :class="{ active: store.filters.favorite }" @click="store.toggleFavoriteOnly">
      ★ 仅收藏
    </button>
  </div>

  <div v-if="managing" class="banner" style="margin-bottom: 14px">
    <span>🧹</span>
    <div class="row tight" style="flex: 1">
      <strong>已选 {{ selectedCount }} 条</strong>
      <button class="btn sm ghost" @click="toggleSelectAll">
        {{ allSelected ? "取消全选" : "全选本页" }}
      </button>
      <div class="spacer" />
      <template v-if="!deleteArmed">
        <button
          class="btn sm danger"
          :disabled="selectedCount === 0 || deleting"
          @click="deleteSelected"
        >
          删除选中
        </button>
      </template>
      <template v-else>
        <span class="hint">只删本地数据；来源里还在的下次导入会重新加回来</span>
        <button class="btn sm danger" :disabled="deleting" @click="deleteSelected">
          {{ deleting ? "删除中…" : `确认删除 ${selectedCount} 条` }}
        </button>
        <button class="btn sm ghost" :disabled="deleting" @click="deleteArmed = false">取消</button>
      </template>
    </div>
  </div>

  <div v-if="store.error" class="banner danger" style="margin-bottom: 14px">
    <span>✕</span>
    <div>{{ store.error }}</div>
  </div>

  <div v-if="store.loading" style="margin-bottom: 14px" class="loading-bar" />

  <div v-if="store.items.length" class="game-grid">
    <GameCard
      v-for="item in store.items"
      :key="item.game.id"
      :item="item"
      :selectable="managing"
      :selected="selectedIds.has(item.game.id)"
      @open="openDetail"
      @favorite="(target) => store.toggleFavorite(target)"
      @select="toggleSelect"
    />
  </div>

  <div v-else-if="!store.loading" class="empty">
    <template v-if="store.filters.q || store.filters.status || store.filters.favorite">
      没有符合条件的条目，试试清空筛选
    </template>
    <template v-else> 游戏库还是空的，去 <RouterLink to="/import">导入 Steam 愿望单</RouterLink> </template>
  </div>

  <Pagination
    v-if="store.total > store.filters.per_page"
    :page="store.filters.page"
    :total-pages="store.totalPages"
    :total="store.total"
    :per-page="store.filters.per_page"
    @go="store.goPage"
  />

  <GameDetailDrawer
    :game-id="openId"
    @close="closeDetail"
    @updated="(updated) => store.replaceItem(updated)"
    @deleted="onDeleted"
  />
</template>
