<script setup lang="ts">
/** 看板：总量、分布、愿望单价值、最近加入、在玩、同步历史。 */
import { computed, onMounted, ref } from "vue";
import { RouterLink } from "vue-router";

import { enrich, getStats } from "../api";
import type { EnrichSummary, Stats } from "../api/types";
import GameCard from "../components/GameCard.vue";
import StatusBadge from "../components/StatusBadge.vue";
import { errorText, pushToast } from "../composables/toast";
import {
  formatDateTime,
  formatMinutes,
  formatNumber,
  formatPrice,
  relativeTime,
  statusLabel,
} from "../composables/format";
import { useMetaStore } from "../stores/meta";

const stats = ref<Stats | null>(null);
const loading = ref(false);
const enriching = ref(false);
const error = ref("");
const meta = useMetaStore();

const statusCount = (status: string) =>
  stats.value?.by_status.find((row) => row.status === status)?.count ?? 0;

const wishlistValue = computed(() => stats.value?.wishlist_value?.[0] ?? null);
const wishlistSaved = computed(() => {
  if (!wishlistValue.value) return 0;
  return Math.max(0, wishlistValue.value.list_price_cents - wishlistValue.value.current_price_cents);
});

const statusBars = computed(() => {
  const total = Math.max(stats.value?.total_games ?? 0, 1);
  return (stats.value?.by_status ?? []).map((row) => ({
    ...row,
    percent: Math.round((row.count / total) * 100),
  }));
});

async function load() {
  loading.value = true;
  error.value = "";
  try {
    stats.value = await getStats();
  } catch (err) {
    error.value = err instanceof Error ? err.message : String(err);
  } finally {
    loading.value = false;
  }
}

async function runEnrich(limit = 20) {
  enriching.value = true;
  try {
    const result: EnrichSummary = await enrich(limit);
    pushToast(
      result.succeeded > 0 ? "ok" : "warn",
      "元数据补全完成",
      `成功 ${result.succeeded} / 失败 ${result.failed}（${result.duration_ms}ms）`,
    );
    await load();
  } catch (err) {
    const { title, message } = errorText(err);
    pushToast("err", title, message);
  } finally {
    enriching.value = false;
  }
}

onMounted(() => {
  void meta.load();
  void load();
});
</script>

<template>
  <div class="page-head">
    <div>
      <h1>看板</h1>
      <p class="sub">
        数据源：{{ meta.steamModeLabel }} · 条目总数 {{ formatNumber(stats?.total_games ?? 0) }}
      </p>
    </div>
    <div class="row">
      <button class="btn" :disabled="enriching" @click="runEnrich(20)">
        {{ enriching ? "补全中…" : "补全 20 条元数据" }}
      </button>
      <button class="btn ghost" :disabled="loading" @click="load">刷新</button>
    </div>
  </div>

  <div v-if="error" class="banner danger">
    <span>✕</span>
    <div>{{ error }}</div>
  </div>

  <div v-if="!stats && loading" class="panel">正在读取统计…</div>

  <template v-if="stats">
    <div class="grid cols-4">
      <div class="stat-card">
        <div class="value">{{ formatNumber(stats.total_games) }}</div>
        <div class="label">条目总数</div>
        <div class="foot">收藏 {{ stats.favorite_count }} 个</div>
      </div>
      <div class="stat-card">
        <div class="value" style="color: var(--status-wishlist)">{{ statusCount("wishlist") }}</div>
        <div class="label">愿望单</div>
        <div class="foot">
          <template v-if="wishlistValue">
            合计 {{ formatPrice(wishlistValue.current_price_cents, wishlistValue.currency) }}
            <span v-if="wishlistSaved > 0">（省 {{ formatPrice(wishlistSaved, wishlistValue.currency) }}）</span>
          </template>
          <template v-else>暂无价格数据</template>
        </div>
      </div>
      <div class="stat-card">
        <div class="value" style="color: var(--status-playing)">{{ statusCount("playing") }}</div>
        <div class="label">在玩</div>
        <div class="foot">已通关 {{ statusCount("completed") }} 个</div>
      </div>
      <div class="stat-card">
        <div class="value">{{ formatMinutes(stats.total_playtime_minutes) }}</div>
        <div class="label">累计时长</div>
        <div class="foot">
          平均评分 {{ stats.average_rating ? stats.average_rating.toFixed(1) : "—" }}
        </div>
      </div>
    </div>

    <div class="panel">
      <div class="panel-head">
        <div>
          <h2>状态分布</h2>
          <div class="desc">按条目数占比</div>
        </div>
        <RouterLink class="btn sm ghost" to="/library">进入游戏库 →</RouterLink>
      </div>
      <div class="grid cols-4">
        <div v-for="row in statusBars" :key="row.status" class="stat-card" style="padding: 12px 14px">
          <div class="row tight" style="justify-content: space-between">
            <StatusBadge :status="row.status" />
            <strong>{{ row.count }}</strong>
          </div>
          <div
            style="height: 4px; border-radius: 999px; background: var(--panel-3); margin-top: 8px; overflow: hidden"
          >
            <div
              :style="{
                width: `${row.percent}%`,
                height: '100%',
                background: `var(--status-${row.status})`,
              }"
            />
          </div>
          <div class="hint" style="margin-top: 4px">{{ row.percent }}% · {{ statusLabel(row.status) }}</div>
        </div>
      </div>
    </div>

    <div class="panel">
      <div class="panel-head">
        <div>
          <h2>最近加入</h2>
          <div class="desc">按「加入愿望单 / 入库时间」排序</div>
        </div>
      </div>
      <div v-if="stats.recently_added.length" class="game-grid">
        <GameCard
          v-for="item in stats.recently_added"
          :key="item.game.id"
          :item="item"
          @open="$router.push(`/library?focus=${item.game.id}`)"
        />
      </div>
      <div v-else class="empty">
        还没有数据，去 <RouterLink to="/import">导入 Steam 愿望单</RouterLink> 开始吧
      </div>
    </div>

    <div class="grid cols-2">
      <div class="panel">
        <div class="panel-head">
          <h2>在玩</h2>
        </div>
        <div v-if="stats.playing.length" class="row" style="flex-direction: column; align-items: stretch">
          <div v-for="item in stats.playing" :key="item.game.id" class="row tight">
            <StatusBadge :status="item.entry.status" />
            <strong>{{ item.game.title_zh || item.game.title }}</strong>
            <div class="spacer" />
            <span class="hint">
              {{ item.entry.progress_percent ?? 0 }}% · {{ formatMinutes(item.entry.playtime_minutes) }}
            </span>
          </div>
        </div>
        <div v-else class="empty">还没有「在玩」的条目</div>
      </div>

      <div class="panel">
        <div class="panel-head">
          <div>
            <h2>同步历史</h2>
            <div class="desc">最近 6 次导入 / 补全</div>
          </div>
          <RouterLink class="btn sm ghost" to="/import">全部记录 →</RouterLink>
        </div>
        <table v-if="stats.recent_imports.length" class="table">
          <thead>
            <tr>
              <th>时间</th>
              <th>类型</th>
              <th>新增</th>
              <th>更新</th>
              <th>状态</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="run in stats.recent_imports" :key="run.id">
              <td :title="formatDateTime(run.started_at)">{{ relativeTime(run.started_at) }}</td>
              <td>{{ run.mode }}</td>
              <td>{{ run.added }}</td>
              <td>{{ run.updated }}</td>
              <td>
                <span class="badge" :class="run.status === 'ok' ? 'ok' : run.status === 'failed' ? 'danger' : 'warn'">
                  {{ run.status }}
                </span>
              </td>
            </tr>
          </tbody>
        </table>
        <div v-else class="empty">暂无同步记录</div>
      </div>
    </div>
  </template>
</template>
