<script setup lang="ts">
/** 游戏新闻：已拥有游戏的 Steam 公告，按发布时间倒序。 */
import { computed, onMounted, ref } from "vue";
import { RouterLink } from "vue-router";

import { getNewsStatus, listNews, syncNews } from "../api";
import type { GameNews, NewsStatus, Page } from "../api/types";
import Pagination from "../components/Pagination.vue";
import PosterImage from "../components/PosterImage.vue";
import { formatDateTime, relativeTime } from "../composables/format";
import { errorText, pushToast } from "../composables/toast";
import { useMetaStore } from "../stores/meta";

const meta = useMetaStore();
const status = ref<NewsStatus | null>(null);
const page = ref<Page<GameNews> | null>(null);
const loading = ref(false);
const syncing = ref(false);
const error = ref("");
const batchSize = ref(30);
const perPage = 20;
const currentPage = ref(1);

const items = computed(() => page.value?.items ?? []);
const total = computed(() => page.value?.total ?? 0);
const totalPages = computed(() => page.value?.total_pages ?? 1);
const hasOwned = computed(() => (status.value?.owned_games ?? 0) > 0);
const pending = computed(() => status.value?.pending_games ?? 0);

function gameLabel(item: GameNews): string {
  return item.game_title_zh?.trim() || item.game_title?.trim() || `#${item.external_id}`;
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

async function sync() {
  if (syncing.value) return;
  syncing.value = true;
  try {
    const result = await syncNews({ limit: batchSize.value });
    const tone =
      result.games_failed > 0 ? "warn" : result.items_added + result.items_updated > 0 ? "ok" : "warn";
    const detail = [
      `检查 ${result.games_checked} 个游戏`,
      `新增 ${result.items_added} / 更新 ${result.items_updated} / 无变化 ${result.items_unchanged}`,
      result.games_failed > 0 ? `失败 ${result.games_failed}` : "",
      result.pending_games > 0 ? `还剩 ${result.pending_games} 个待同步` : "",
    ]
      .filter(Boolean)
      .join(" · ");
    pushToast(tone, "新闻同步完成", detail);
    await load(1);
  } catch (err) {
    const { title, message } = errorText(err);
    pushToast("err", title, message);
  } finally {
    syncing.value = false;
  }
}

async function goPage(target: number) {
  if (target === currentPage.value) return;
  await load(target);
}

onMounted(() => {
  void meta.load();
  void load(1);
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
      <select v-model.number="batchSize" class="select" style="width: 110px" :disabled="syncing">
        <option :value="10">10 个</option>
        <option :value="30">30 个</option>
        <option :value="50">50 个</option>
        <option :value="100">100 个</option>
      </select>
      <button class="btn primary" :disabled="syncing || !hasOwned" @click="sync">
        {{ syncing ? "同步中…" : "同步新闻" }}
      </button>
      <button class="btn ghost" :disabled="loading || syncing" @click="load(currentPage)">刷新</button>
    </div>
  </div>

  <div v-if="error" class="banner danger" style="margin-bottom: 14px">
    <span>✕</span>
    <div>{{ error }}</div>
  </div>

  <div v-if="status && hasOwned && pending > 0" class="banner" style="margin-bottom: 14px">
    <span>📰</span>
    <div>
      还有 {{ pending }} 个已拥有的游戏没有同步过新闻。每次可以同步一批，从没抓过 / 上次失败的
      优先；已抓过的会按最久未更新的顺序轮换。
    </div>
    <div class="spacer" />
    <button class="btn sm" :disabled="syncing" @click="sync">
      {{ syncing ? "同步中…" : `同步下一批` }}
    </button>
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
    <button class="btn sm" :disabled="syncing" @click="sync">重试</button>
  </div>

  <div v-if="loading && !items.length" class="panel">正在读取新闻…</div>

  <div v-if="items.length" class="news-list">
    <article v-for="item in items" :key="item.id" class="news-card">
      <RouterLink
        class="poster news-cover"
        :to="`/library?focus=${item.game_id}`"
        :title="gameLabel(item)"
      >
        <PosterImage
          :src="item.game_poster_url"
          :fallback-src="item.game_cover_url"
          :alt="gameLabel(item)"
        />
      </RouterLink>

      <div class="news-body">
        <div class="row tight news-meta">
          <RouterLink class="badge" :to="`/library?focus=${item.game_id}`">
            {{ gameLabel(item) }}
          </RouterLink>
          <span class="hint" :title="formatDateTime(item.published_at)">
            {{ relativeTime(item.published_at) }}
          </span>
          <span v-if="item.author" class="hint">· {{ item.author }}</span>
        </div>

        <h2 class="news-title">
          <a :href="item.url" target="_blank" rel="noopener noreferrer">{{ item.title }}</a>
        </h2>
        <p v-if="item.summary" class="news-summary">{{ item.summary }}</p>

        <div class="row tight">
          <a class="hint" :href="item.url" target="_blank" rel="noopener noreferrer">
            查看 Steam 公告原文 →
          </a>
          <span class="spacer" />
          <span class="hint" :title="formatDateTime(item.fetched_at)">
            首次抓取 {{ formatDateTime(item.fetched_at) }}
          </span>
        </div>
      </div>
    </article>
  </div>

  <div v-else-if="!loading" class="empty">
    <template v-if="!hasOwned">
      还没有已拥有的 Steam 游戏。先去
      <RouterLink to="/import">导入与同步</RouterLink>
      里同步库存，新闻只会从已拥有的游戏抓取。
    </template>
    <template v-else>
      还没有新闻。点右上角「同步新闻」抓取下一批已拥有游戏的公告。
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
</template>
