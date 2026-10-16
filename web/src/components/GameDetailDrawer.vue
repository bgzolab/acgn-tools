<script setup lang="ts">
/** 条目详情抽屉：改状态 / 评分 / 进度 / 备注，并展示完整元数据。 */
import { computed, onMounted, onUnmounted, ref, watch } from "vue";

import { getGame, patchGame, refreshMetadata } from "../api";
import {
  STATUS_LABELS,
  STATUS_ORDER,
  type EntryPatch,
  type GameWithEntry,
  type Status,
} from "../api/types";import {
  displayTitle,
  formatDateTime,
  formatMinutes,
  formatNumber,
  formatPrice,
  mediaKindLabel,
  relativeTime,
  reviewColor,
  reviewLabelZh,
  secondaryTitle,
  sourceLabel,
} from "../composables/format";
import { errorText, pushToast } from "../composables/toast";

const props = defineProps<{ gameId: number | null }>();
const emit = defineEmits<{ close: []; updated: [item: GameWithEntry] }>();

const item = ref<GameWithEntry | null>(null);
const loading = ref(false);
const saving = ref(false);
const refreshing = ref(false);
const noteDraft = ref("");
/** 以「分钟」为编辑单位：小时 round-trip 会把 121 分钟悄悄写成 120 分钟 */
const playtimeMinutes = ref<string>("");

const game = computed(() => item.value?.game ?? null);
const entry = computed(() => item.value?.entry ?? null);
const title = computed(() => (game.value ? displayTitle(game.value) : ""));
const subtitle = computed(() => (game.value ? secondaryTitle(game.value) : ""));

const playtimeHint = computed(() => {
  const minutes = Number.parseInt(playtimeMinutes.value, 10);
  if (!Number.isFinite(minutes) || minutes <= 0) return "留空并保存可清空时长";
  return `≈ ${(minutes / 60).toFixed(1)} 小时`;
});

async function load(id: number) {
  loading.value = true;
  try {
    const loaded = await getGame(id);
    item.value = loaded;
    noteDraft.value = loaded.entry.note ?? "";
    playtimeMinutes.value =
      loaded.entry.playtime_minutes && loaded.entry.playtime_minutes > 0
        ? String(loaded.entry.playtime_minutes)
        : "";
  } catch (error) {
    const { title: toastTitle, message } = errorText(error);
    pushToast("err", toastTitle, message);
    emit("close");
  } finally {
    loading.value = false;
  }
}

/**
 * 提交补丁。成功返回更新后的条目，失败返回 `null`
 * （错误 toast 已在此处弹过，调用方不要无条件再弹「已保存」）。
 */
async function patch(patchBody: EntryPatch): Promise<GameWithEntry | null> {
  if (!item.value) return null;
  saving.value = true;
  try {
    const updated = await patchGame(item.value.game.id, patchBody);
    item.value = updated;
    emit("updated", updated);
    return updated;
  } catch (error) {
    const { title: toastTitle, message } = errorText(error);
    pushToast("err", toastTitle, message);
    return null;
  } finally {
    saving.value = false;
  }
}

function setStatus(status: Status) {
  if (!entry.value || entry.value.status === status) return;
  void patch({ status });
}

function toggleFavorite() {
  if (!entry.value) return;
  void patch({ favorite: !entry.value.favorite });
}

function setRating(value: number) {
  if (!entry.value) return;
  const next = entry.value.rating === value ? -1 : value;
  void patch({ rating: next });
}

function setProgress(value: number) {
  void patch({ progress_percent: value });
}

async function saveNoteAndPlaytime() {
  if (!item.value) return;

  const patchBody: EntryPatch = { note: noteDraft.value };
  const raw = playtimeMinutes.value.trim();

  if (raw === "") {
    // 留空 = 清空时长
    if (item.value.entry.playtime_minutes !== null) patchBody.playtime_minutes = -1;
  } else {
    const minutes = Number.parseInt(raw, 10);
    if (!Number.isFinite(minutes) || minutes < 0) {
      pushToast("warn", "时长不合法", "请填写非负整数（分钟），或留空以清空");
      return;
    }
    // 只在真的变化时才提交，避免无谓写入
    if (minutes !== (item.value.entry.playtime_minutes ?? -1)) {
      patchBody.playtime_minutes = minutes;
    }
  }

  const saved = await patch(patchBody);
  // 只有真的保存成功才提示「已保存」：失败时 patch() 已经弹过错误 toast
  if (saved) pushToast("ok", "已保存", "备注与时长已更新");
}

async function doRefresh() {
  if (!game.value) return;
  refreshing.value = true;
  try {
    const result = await refreshMetadata(game.value.id);
    item.value = { game: result.game, entry: result.entry };
    emit("updated", item.value);
    pushToast("ok", "元数据已更新", `来源结果：${result.outcome}`);
  } catch (error) {
    const { title: toastTitle, message } = errorText(error);
    pushToast("err", toastTitle, message);
  } finally {
    refreshing.value = false;
  }
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") emit("close");
}

watch(
  () => props.gameId,
  (id) => {
    if (id !== null) void load(id);
    else item.value = null;
  },
);

onMounted(() => window.addEventListener("keydown", onKeydown));
onUnmounted(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <div v-if="props.gameId !== null" class="drawer-mask" @click="emit('close')" />
  <aside v-if="props.gameId !== null" class="drawer">
    <div
      class="drawer-hero"
      :style="game?.cover_url ? { backgroundImage: `url('${game.cover_url}')` } : undefined"
    >
      <button class="icon-btn close" title="关闭 (Esc)" @click="emit('close')">×</button>
      <div class="head">
        <div class="row tight">
          <span v-if="game" class="badge">{{ sourceLabel(game.source) }} · {{ mediaKindLabel(game.media_kind) }}</span>
          <span v-if="game?.is_free" class="badge ok">免费</span>
          <span v-if="game?.discount_percent" class="badge discount">-{{ game.discount_percent }}%</span>
        </div>
        <h1 style="margin-top: 6px">{{ title }}</h1>
        <div v-if="subtitle" class="hint">{{ subtitle }}</div>
      </div>
    </div>

    <div class="drawer-body">
      <div v-if="loading" class="empty">载入中…</div>

      <template v-else-if="item && game && entry">
        <!-- 状态 -->
        <div class="panel-head">
          <h2>我的进度</h2>
          <span class="hint">{{ saving ? "保存中…" : `更新于 ${relativeTime(entry.updated_at)}` }}</span>
        </div>

        <div class="row tight" style="margin-bottom: 12px">
          <button
            v-for="status in STATUS_ORDER"
            :key="status"
            class="chip"
            :class="{ active: entry.status === status }"
            @click="setStatus(status)"
          >
            {{ STATUS_LABELS[status] }}
          </button>
          <button class="chip" :class="{ active: entry.favorite }" @click="toggleFavorite">
            ★ {{ entry.favorite ? "已收藏" : "收藏" }}
          </button>
        </div>

        <div class="field">
          <span class="label">评分（点击已选分数可清除）</span>
          <div class="stars">
            <button
              v-for="score in 10"
              :key="score"
              class="star"
              :class="{ on: (entry.rating ?? 0) >= score }"
              :title="`${score} 分`"
              @click="setRating(score)"
            >
              {{ score }}
            </button>
          </div>
        </div>

        <div class="field">
          <span class="label">进度：{{ entry.progress_percent ?? 0 }}%</span>
          <input
            type="range"
            min="0"
            max="100"
            step="5"
            :value="entry.progress_percent ?? 0"
            style="width: 100%; accent-color: var(--accent)"
            @change="setProgress(Number(($event.target as HTMLInputElement).value))"
          />
        </div>

        <div class="field">
          <label class="label" for="playtime">游玩时长（分钟）</label>
          <input
            id="playtime"
            v-model="playtimeMinutes"
            class="input"
            inputmode="numeric"
            placeholder="例如 750"
          />
          <div class="hint" style="margin-top: 4px">{{ playtimeHint }}</div>
        </div>

        <div class="field">
          <label class="label" for="note">备注</label>
          <textarea id="note" v-model="noteDraft" class="textarea" placeholder="为什么想玩 / 卡在哪了 / 通关感想…" />
        </div>

        <div class="row">
          <button class="btn primary" @click="saveNoteAndPlaytime">保存备注与时长</button>
          <button class="btn" :disabled="refreshing" @click="doRefresh">
            {{ refreshing ? "抓取中…" : "刷新商店元数据" }}
          </button>
          <a
            v-if="game.source === 'steam'"
            class="btn ghost"
            :href="`https://store.steampowered.com/app/${game.external_id}/`"
            target="_blank"
            rel="noreferrer"
            >在 Steam 打开 ↗</a
          >
        </div>

        <div class="divider" />

        <!-- 元数据 -->
        <div class="panel-head">
          <h2>条目信息</h2>
          <span class="hint">
            {{ game.metadata_updated_at ? `元数据抓取于 ${relativeTime(game.metadata_updated_at)}` : "元数据未补全" }}
          </span>
        </div>

        <p v-if="game.short_description" style="color: var(--text-dim); font-size: 12.5px; margin-bottom: 10px">
          {{ game.short_description }}
        </p>

        <dl class="kv">
          <dt>好评</dt>
          <dd :style="{ color: reviewColor(game.review_score) }">
            {{ reviewLabelZh(game) }}
            <span v-if="game.review_count">（{{ formatNumber(game.review_count) }} 条）</span>
          </dd>

          <dt>发售</dt>
          <dd>{{ game.release_raw || game.release_date || "—" }}</dd>

          <dt>价格</dt>
          <dd>
            {{ formatPrice(game.price_final ?? game.price_initial, game.price_currency) }}
            <span v-if="game.discount_percent" class="hint">
              （原价 {{ formatPrice(game.price_initial, game.price_currency) }}）
            </span>
          </dd>

          <dt>开发商</dt>
          <dd>{{ game.developers.join(" / ") || "—" }}</dd>

          <dt>发行商</dt>
          <dd>{{ game.publishers.join(" / ") || "—" }}</dd>

          <dt>类型</dt>
          <dd>{{ game.genres.join(" · ") || "—" }}</dd>

          <dt>平台</dt>
          <dd>{{ game.platforms.join(" · ") || "—" }}</dd>

          <dt v-if="game.tags.length">标签</dt>
          <dd v-if="game.tags.length">{{ game.tags.join(" · ") }}</dd>

          <dt>加入时间</dt>
          <dd>{{ formatDateTime(entry.source_added_at) }}（{{ relativeTime(entry.source_added_at) }}）</dd>

          <dt>最近同步</dt>
          <dd>{{ formatDateTime(entry.last_synced_at) }}</dd>

          <dt v-if="entry.playtime_minutes">时长</dt>
          <dd v-if="entry.playtime_minutes">{{ formatMinutes(entry.playtime_minutes) }}</dd>

          <dt>来源 ID</dt>
          <dd>{{ game.source }} / {{ game.external_id }}</dd>
        </dl>
      </template>
    </div>
  </aside>
</template>
