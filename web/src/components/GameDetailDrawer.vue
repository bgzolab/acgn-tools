<script setup lang="ts">
/** 条目详情抽屉：改状态 / 评分 / 进度 / 备注，并展示完整元数据。 */
import { computed, onMounted, onUnmounted, ref, watch } from "vue";

import { deleteGame, getGame, patchGame, refreshMetadata } from "../api";
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
const emit = defineEmits<{ close: []; updated: [item: GameWithEntry]; deleted: [id: number] }>();

const item = ref<GameWithEntry | null>(null);
const loading = ref(false);
const saving = ref(false);
const refreshing = ref(false);
/** 删除按钮的二段确认：先「删除条目」，再「确认删除」 */
const deleteArmed = ref(false);
const deleting = ref(false);
const noteDraft = ref("");
/** 以「分钟」为编辑单位：小时 round-trip 会把 121 分钟悄悄写成 120 分钟 */
const playtimeMinutes = ref<string>("");

const game = computed(() => item.value?.game ?? null);
const entry = computed(() => item.value?.entry ?? null);
const title = computed(() => (game.value ? displayTitle(game.value) : ""));
const subtitle = computed(() => (game.value ? secondaryTitle(game.value) : ""));

/** Steam 记录的时长（来源跟踪值） */
const sourcePlaytime = computed(() => entry.value?.source_playtime_minutes ?? null);
/** 当前有效时长是否仍在跟随来源（未被本地手填过） */
const followsSource = computed(() => {
  const current = entry.value;
  if (!current || current.source_playtime_minutes === null) return false;
  return (
    current.playtime_minutes === null ||
    current.playtime_minutes === current.source_playtime_minutes
  );
});

/** Steam 记录的可读文案（0 = 尚未游玩） */
const sourcePlaytimeLabel = computed(() =>
  sourcePlaytime.value !== null && sourcePlaytime.value > 0
    ? formatMinutes(sourcePlaytime.value)
    : "尚未游玩",
);

const playtimeHint = computed(() => {
  const raw = playtimeMinutes.value.trim();
  const source = sourcePlaytime.value;
  if (raw === "") {
    if (source !== null) {
      return `留空并保存 = 恢复跟随 Steam 时长（${sourcePlaytimeLabel.value}）`;
    }
    return "留空并保存可清空时长";
  }
  const minutes = Number.parseInt(raw, 10);
  if (!Number.isFinite(minutes) || minutes < 0) {
    return "请填写非负整数（分钟），或留空以清空";
  }
  if (minutes === 0) {
    // 0 是「手动记 0 分钟」，不是「恢复跟随」：两条路的语义不同，提示要写清楚
    return source !== null
      ? "保存 0 = 手动记为 0 分钟（不再跟随 Steam）"
      : "保存 0 = 手动记为 0 分钟";
  }
  const base = `≈ ${(minutes / 60).toFixed(1)} 小时`;
  if (source !== null && minutes !== source) {
    return `${base} · Steam 记录 ${sourcePlaytimeLabel.value}，保存后以本地为准`;
  }
  return base;
});

async function load(id: number) {
  loading.value = true;
  try {
    const loaded = await getGame(id);
    // 慢响应守卫：等待期间抽屉可能已经切到别的条目。没有这道守卫时，
    // A 的迟到响应会把 item 覆盖成 A 的数据，而 props.gameId 已经是 B——
    // 用户随后改备注／评分就会 PATCH 到 A 上（静默错写）。
    if (props.gameId !== id) return;
    item.value = loaded;
    noteDraft.value = loaded.entry.note ?? "";
    playtimeMinutes.value =
      loaded.entry.playtime_minutes && loaded.entry.playtime_minutes > 0
        ? String(loaded.entry.playtime_minutes)
        : "";
  } catch (error) {
    if (props.gameId !== id) return;
    const { title: toastTitle, message } = errorText(error);
    pushToast("err", toastTitle, message);
    emit("close");
  } finally {
    if (props.gameId === id) loading.value = false;
  }
}

/**
 * 提交补丁。成功返回更新后的条目，失败返回 `null`
 * （错误 toast 已在此处弹过，调用方不要无条件再弹「已保存」）。
 */
async function patch(patchBody: EntryPatch): Promise<GameWithEntry | null> {
  if (!item.value) return null;
  // 记下这条 PATCH 的目标：写入期间抽屉可能被切走，迟到响应不能覆盖当前内容
  const targetId = item.value.game.id;
  saving.value = true;
  try {
    const updated = await patchGame(targetId, patchBody);
    if (props.gameId !== targetId) return null;
    item.value = updated;
    emit("updated", updated);
    return updated;
  } catch (error) {
    if (props.gameId !== targetId) return null;
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
    // 留空 = 清空本地手填值；有 Steam 记录时后端会恢复为来源值
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

/** 一键采用 Steam 时长：等于来源值后恢复「自动跟随」 */
async function adoptSourcePlaytime() {
  const source = entry.value?.source_playtime_minutes;
  if (source === null || source === undefined) return;
  const saved = await patch({ playtime_minutes: source });
  if (saved) {
    playtimeMinutes.value = source > 0 ? String(source) : "";
    pushToast("ok", "已采用 Steam 时长", "之后库存同步会自动跟随");
  }
}

async function doRefresh() {
  if (!game.value) return;
  const targetId = game.value.id;
  refreshing.value = true;
  try {
    const result = await refreshMetadata(targetId);
    // 同样的慢响应守卫：切走了就不要把结果盖回去
    if (props.gameId !== targetId) return;
    item.value = { game: result.game, entry: result.entry };
    emit("updated", item.value);
    pushToast("ok", "元数据已更新", `来源结果：${result.outcome}`);
  } catch (error) {
    if (props.gameId !== targetId) return;
    const { title: toastTitle, message } = errorText(error);
    pushToast("err", toastTitle, message);
  } finally {
    refreshing.value = false;
  }
}

async function doDelete() {
  if (!item.value || deleting.value) return;
  const targetId = item.value.game.id;
  const label = title.value;
  deleting.value = true;
  try {
    await deleteGame(targetId);
    pushToast("ok", "已删除", `「${label}」已从本地移除`);
    emit("deleted", targetId);
  } catch (error) {
    const { title: toastTitle, message } = errorText(error);
    pushToast("err", toastTitle, message);
  } finally {
    deleting.value = false;
  }
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") emit("close");
}

watch(
  () => props.gameId,
  (id) => {
    deleteArmed.value = false;
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
          <div
            v-if="sourcePlaytime !== null && !followsSource"
            class="row tight"
            style="margin-top: 6px"
          >
            <span class="hint">Steam 记录：{{ sourcePlaytimeLabel }}</span>
            <button class="btn sm ghost" :disabled="saving" @click="adoptSourcePlaytime">
              采用 Steam 时长
            </button>
          </div>
          <div v-else-if="sourcePlaytime !== null" class="hint" style="margin-top: 4px">
            当前跟随 Steam 时长（{{ sourcePlaytimeLabel }}）
          </div>
        </div>

        <div v-if="entry.achievements_total !== null" class="field">
          <span class="label">成就</span>
          <div class="hint">
            <template v-if="entry.achievements_total === 0">该游戏没有成就</template>
            <template v-else>
              {{ entry.achievements_achieved ?? 0 }} / {{ entry.achievements_total }}
              <span
                v-if="(entry.achievements_achieved ?? 0) >= entry.achievements_total"
                class="badge ok"
                style="margin-left: 4px"
                >全成就</span
              >
            </template>
            <span class="hint" style="margin-left: 6px">
              （检查于 {{ relativeTime(entry.achievements_checked_at) }}）
            </span>
          </div>
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
          <dd v-if="entry.playtime_minutes">
            {{ formatMinutes(entry.playtime_minutes) }}
            <span
              v-if="
                entry.source_playtime_minutes !== null &&
                entry.playtime_minutes !== entry.source_playtime_minutes
              "
              class="hint"
            >
              （本地手填；Steam 记录 {{ formatMinutes(entry.source_playtime_minutes) }}）
            </span>
          </dd>

          <dt>来源 ID</dt>
          <dd>{{ game.source }} / {{ game.external_id }}</dd>
        </dl>

        <div class="divider" />

        <!-- 危险区：删除是本地动作，来源里还在的下次导入会重新加回来 -->
        <div class="row tight">
          <template v-if="!deleteArmed">
            <button class="btn danger sm" @click="deleteArmed = true">删除条目</button>
            <span class="hint">只删本地数据；来源里还有的条目，下次导入会重新加回来</span>
          </template>
          <template v-else>
            <span class="hint">删除后个人状态（评分 / 备注 / 时长）一并丢失，确定？</span>
            <div class="spacer" />
            <button class="btn danger sm" :disabled="deleting" @click="doDelete">
              {{ deleting ? "删除中…" : "确认删除" }}
            </button>
            <button class="btn ghost sm" :disabled="deleting" @click="deleteArmed = false">取消</button>
          </template>
        </div>
      </template>
    </div>
  </aside>
</template>
