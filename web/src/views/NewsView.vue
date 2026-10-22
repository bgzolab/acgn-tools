<script setup lang="ts">
/** 游戏新闻：已拥有游戏的 Steam 公告，按发布时间倒序；同步是后台任务，边抓边刷新。 */
import { computed, onMounted, onUnmounted, ref } from "vue";
import { RouterLink } from "vue-router";

import { getNewsStatus, getNewsSyncStatus, listNews, startNewsSync } from "../api";
import type { GameNews, NewsStatus, NewsSyncState, Page } from "../api/types";
import NewsDetailModal from "../components/NewsDetailModal.vue";
import Pagination from "../components/Pagination.vue";
import PosterImage from "../components/PosterImage.vue";
import { formatDateTime, relativeTime } from "../composables/format";
import { errorText, pushToast } from "../composables/toast";
import { useMetaStore } from "../stores/meta";

const meta = useMetaStore();
const status = ref<NewsStatus | null>(null);
const page = ref<Page<GameNews> | null>(null);
const syncState = ref<NewsSyncState | null>(null);
const loading = ref(false);
const error = ref("");
const pollError = ref("");
const perPage = 20;
const currentPage = ref(1);
const selected = ref<GameNews | null>(null);
const starting = ref(false);

let syncTimer: number | undefined;
let refreshing = false;
let disposed = false;
/** 单调递增：startSync / 卸载 / 重新挂载都会让旧的异步轮询失效。 */
let syncGeneration = 0;

const items = computed(() => page.value?.items ?? []);
const total = computed(() => page.value?.total ?? 0);
const totalPages = computed(() => page.value?.total_pages ?? 1);
const hasOwned = computed(() => (status.value?.owned_games ?? 0) > 0);
const pending = computed(() => status.value?.pending_games ?? 0);
const syncing = computed(() => starting.value || (syncState.value?.running ?? false));

const progressPercent = computed(() => {
  const state = syncState.value;
  if (!state || state.total <= 0) return 0;
  return Math.min(100, Math.round((state.done / state.total) * 100));
});

const etaText = computed(() => formatEta(syncState.value?.eta_seconds));
const progressTitle = computed(() => {
  const state = syncState.value;
  if (!state) return "";
  return `${state.done} / ${state.total} 个游戏`;
});

function gameLabel(item: GameNews): string {
  return item.game_title_zh?.trim() || item.game_title?.trim() || `#${item.external_id}`;
}

function formatDuration(ms: number | null | undefined): string {
  const seconds = Math.max(0, Math.round((ms ?? 0) / 1000));
  if (seconds < 60) return `${seconds} 秒`;
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return `${minutes} 分 ${rest} 秒`;
}

function formatEta(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return "预计剩余计算中…";
  if (seconds <= 0) return "即将完成";
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return minutes > 0 ? `预计剩余约 ${minutes} 分 ${rest} 秒` : `预计剩余约 ${rest} 秒`;
}

async function load(targetPage = currentPage.value) {
  loading.value = true;
  error.value = "";
  try {
    const [nextStatus, nextPage] = await Promise.all([
      getNewsStatus(),
      listNews({ page: targetPage, per_page: perPage }),
    ]);
    status.value = nextStatus;
    page.value = nextPage;
    currentPage.value = nextPage.page;
  } catch (err) {
    error.value = err instanceof Error ? err.message : String(err);
  } finally {
    loading.value = false;
  }
}

/** 同步过程中静默刷新：不显示整页 loading，失败时保留已有列表。 */
async function refreshWhileSyncing() {
  if (refreshing) return;
  refreshing = true;
  try {
    const [nextStatus, nextPage] = await Promise.all([
      getNewsStatus(),
      listNews({ page: currentPage.value, per_page: perPage }),
    ]);
    status.value = nextStatus;
    page.value = nextPage;
    currentPage.value = nextPage.page;
  } catch {
    // 轮询同一条链路，网络抖动时下一次会补上，不打断阅读。
  } finally {
    refreshing = false;
  }
}

function stopPolling() {
  syncGeneration += 1;
  if (syncTimer !== undefined) {
    window.clearTimeout(syncTimer);
    syncTimer = undefined;
  }
}

function schedulePoll(generation: number, delayMs = 600) {
  if (disposed || generation !== syncGeneration) return;
  syncTimer = window.setTimeout(() => void pollSync(generation), delayMs);
}

function finishToast(state: NewsSyncState) {
  const tone = state.failed > 0 ? "warn" : "ok";
  const parts = [
    `检查 ${state.succeeded} 个游戏`,
    `新增 ${state.items_added} / 更新 ${state.items_updated}`,
    state.failed > 0 ? `失败 ${state.failed}` : "",
    `用时 ${formatDuration(state.elapsed_ms)}`,
  ].filter(Boolean);
  pushToast(tone, "新闻同步完成", parts.join(" · "));
}

async function pollSync(generation: number) {
  if (disposed || generation !== syncGeneration) return;
  try {
    const next = await getNewsSyncStatus();
    // 请求挂起期间可能已开始新一轮 / 已离开页面：旧响应直接丢弃。
    if (disposed || generation !== syncGeneration) return;
    // 轮询恢复后只清轮询自己的错误，不覆盖列表加载错误。
    pollError.value = "";
    const previousDone = syncState.value?.done ?? 0;
    syncState.value = next;

    if (next.done > previousDone) {
      void refreshWhileSyncing();
    }

    if (next.running) {
      schedulePoll(generation, 600);
    } else {
      if (syncTimer !== undefined) {
        window.clearTimeout(syncTimer);
        syncTimer = undefined;
      }
      await load(currentPage.value);
      if (disposed || generation !== syncGeneration) return;
      if (next.done > 0) finishToast(next);
    }
  } catch (err) {
    if (disposed || generation !== syncGeneration) return;
    // 后端短暂不可达时继续轮询，恢复后进度会自己接上。
    const { title, message } = errorText(err);
    pollError.value = `${title}：${message}`;
    schedulePoll(generation, 1500);
  }
}

async function startSync() {
  if (syncing.value) return;
  // 让任何挂起中的旧轮询失效，再开启新一轮。
  stopPolling();
  error.value = "";
  pollError.value = "";
  starting.value = true;
  try {
    const initial = await startNewsSync();
    if (disposed) return;
    syncState.value = initial;
    if (initial.running) {
      // 新条目从第 1 页开始出现；用户仍可继续翻页，轮询会刷新当前页。
      currentPage.value = 1;
      void refreshWhileSyncing();
      schedulePoll(syncGeneration, 600);
    } else {
      await load(currentPage.value);
      const warning = initial.warnings[0] ?? "没有可同步的游戏";
      pushToast("warn", "没有开始同步", warning);
    }
  } catch (err) {
    if (disposed) return;
    const { title, message } = errorText(err);
    pushToast("err", title, message);
  } finally {
    starting.value = false;
  }
}

async function goPage(target: number) {
  if (target === currentPage.value) return;
  await load(target);
}

onMounted(async () => {
  void meta.load();
  const generation = ++syncGeneration;
  await load(1);
  if (disposed || generation !== syncGeneration) return;
  try {
    const current = await getNewsSyncStatus();
    if (disposed || generation !== syncGeneration) return;
    syncState.value = current;
    if (current.running) {
      schedulePoll(generation, 300);
    }
  } catch {
    // 进度接口不可用时不影响已有列表。
  }
});

onUnmounted(() => {
  disposed = true;
  stopPolling();
});
</script>

<template>
  <div class="page-head">
    <div>
      <h1>游戏新闻</h1>
      <p class="sub">
        已拥有 {{ status?.owned_games ?? 0 }} 个游戏 · 已同步 {{ status?.synced_games ?? 0 }} 个
        <template v-if="status?.last_synced_at"> · {{ relativeTime(status.last_synced_at) }}更新</template>
      </p>
    </div>
    <div class="row">
      <button class="btn primary" :disabled="syncing || !hasOwned" @click="startSync">
        {{ syncing ? "同步中…" : "同步全部新闻" }}
      </button>
      <button class="btn ghost" :disabled="loading" @click="load(currentPage)">刷新</button>
    </div>
  </div>

  <div v-if="error" class="banner danger" style="margin-bottom: 14px">
    <span>✕</span>
    <div>{{ error }}</div>
  </div>

  <div v-if="pollError" class="banner warn" style="margin-bottom: 14px">
    <span>⚠</span>
    <div>{{ pollError }}（正在自动重试）</div>
  </div>

  <div v-if="syncState?.running" class="panel sync-panel">
    <div class="panel-head" style="margin-bottom: 8px">
      <div>
        <h2>正在同步全部新闻</h2>
        <div class="desc">
          {{ progressTitle }} · 已用时 {{ formatDuration(syncState.elapsed_ms) }} · {{ etaText }}
        </div>
      </div>
      <div class="hint">
        成功 {{ syncState.succeeded }} · 失败 {{ syncState.failed }}
      </div>
    </div>
    <div class="sync-bar">
      <div class="sync-bar-fill" :style="{ width: `${progressPercent}%` }" />
    </div>
    <div class="row tight" style="margin-top: 8px">
      <span class="hint">当前：{{ syncState.current_game || "准备中…" }}</span>
      <span class="spacer" />
      <span class="hint">
        新增 {{ syncState.items_added }} · 更新 {{ syncState.items_updated }}
      </span>
    </div>
  </div>

  <template v-else>
    <div v-if="status && hasOwned && pending > 0" class="banner" style="margin-bottom: 14px">
      <span>📰</span>
      <div>
        还有 {{ pending }} 个已拥有的游戏没有同步过新闻。点「同步全部新闻」会一次拉完所有游戏，
        抓到一个就更新一份列表，不用分批点。
      </div>
      <div class="spacer" />
      <button class="btn sm" @click="startSync">开始同步</button>
    </div>

    <div
      v-else-if="status && status.failed_games > 0"
      class="banner warn"
      style="margin-bottom: 14px"
    >
      <span>⚠</span>
      <div>
        {{ status.failed_games }} 个新闻源上次请求失败，已入库的新闻仍会展示；再同步一次会重试。
      </div>
      <div class="spacer" />
      <button class="btn sm" @click="startSync">重试</button>
    </div>
  </template>

  <div v-if="syncState?.warnings?.length" class="banner warn" style="margin-bottom: 14px">
    <span>⚠</span>
    <div>
      <div class="hint" style="margin-bottom: 2px">本轮同步告警</div>
      <div v-for="warning in syncState.warnings.slice(0, 5)" :key="warning">{{ warning }}</div>
    </div>
  </div>

  <div v-if="loading && !items.length" class="panel">正在读取新闻…</div>

  <div v-if="items.length" class="news-list">
    <article
      v-for="item in items"
      :key="item.id"
      class="news-card"
      role="button"
      tabindex="0"
      @click="selected = item"
      @keydown.enter="selected = item"
      @keydown.space.prevent="selected = item"
    >
      <div class="poster news-cover">
        <PosterImage
          :src="item.game_poster_url"
          :fallback-src="item.game_cover_url"
          :alt="gameLabel(item)"
        />
      </div>

      <div class="news-body">
        <div class="row tight news-meta">
          <RouterLink
            class="badge"
            :to="`/library?focus=${item.game_id}`"
            @click.stop
            @keydown.stop
          >
            {{ gameLabel(item) }}
          </RouterLink>
          <span class="hint" :title="formatDateTime(item.published_at)">
            {{ relativeTime(item.published_at) }}
          </span>
          <span v-if="item.author" class="hint">· {{ item.author }}</span>
        </div>

        <h2 class="news-title">{{ item.title }}</h2>
        <p v-if="item.summary" class="news-summary">{{ item.summary }}</p>

        <div class="row tight">
          <span class="hint">点击查看全文</span>
          <span class="spacer" />
          <span class="hint" :title="formatDateTime(item.fetched_at)">
            首次抓取 {{ formatDateTime(item.fetched_at) }}
          </span>
        </div>
      </div>
    </article>
  </div>

  <div v-else-if="!loading && !syncing" class="empty">
    <template v-if="!hasOwned">
      还没有已拥有的 Steam 游戏。先去
      <RouterLink to="/import">导入与同步</RouterLink>
      里同步库存，新闻只会从已拥有的游戏抓取。
    </template>
    <template v-else>
      还没有新闻。点右上角「同步全部新闻」拉取所有已拥有游戏的公告。
    </template>
  </div>

  <Pagination
    v-if="total > perPage"
    :page="currentPage"
    :total-pages="totalPages"
    :total="total"
    :per-page="perPage"
    @go="goPage"
  />

  <NewsDetailModal v-if="selected" :item="selected" @close="selected = null" />
</template>
