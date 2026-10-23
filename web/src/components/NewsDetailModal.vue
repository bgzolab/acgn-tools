<script setup lang="ts">
/** 新闻全文弹窗：列表只展示摘要，正文在本地完整阅读。 */
import { onMounted, onUnmounted } from "vue";

import type { GameNews } from "../api/types";
import { formatDateTime } from "../composables/format";

const props = defineProps<{ item: GameNews }>();
const emit = defineEmits<{ close: [] }>();

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") emit("close");
}

function gameLabel(item: GameNews): string {
  return item.game_title_zh?.trim() || item.game_title?.trim() || `#${item.external_id}`;
}

onMounted(() => window.addEventListener("keydown", onKeydown));
onUnmounted(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <Teleport to="body">
    <div class="news-modal-mask" @click.self="emit('close')">
      <section class="news-modal" role="dialog" aria-modal="true" aria-label="新闻全文">
        <header class="news-modal-head">
          <div class="news-modal-heading">
            <div class="row tight">
              <span class="badge">{{ gameLabel(props.item) }}</span>
              <span class="hint" :title="formatDateTime(props.item.published_at)">
                {{ formatDateTime(props.item.published_at) }}
              </span>
              <span v-if="props.item.author" class="hint">· {{ props.item.author }}</span>
            </div>
            <h2>{{ props.item.title }}</h2>
          </div>
          <button class="btn sm ghost" aria-label="关闭" @click="emit('close')">✕</button>
        </header>

        <div class="news-modal-full">
          <template v-if="props.item.summary">{{ props.item.summary }}</template>
          <template v-else>这条公告没有正文，可前往 Steam 查看原文。</template>
        </div>

        <footer class="news-modal-foot">
          <a :href="props.item.url" target="_blank" rel="noopener noreferrer">
            在 Steam 打开原文 →
          </a>
          <span class="spacer" />
          <button class="btn sm" @click="emit('close')">关闭</button>
        </footer>
      </section>
    </div>
  </Teleport>
</template>
