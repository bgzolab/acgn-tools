<script setup lang="ts">
/** 图片 + 兜底链：海报 → 封面 → 文字占位（CDN 被墙 / 旧式海报 404 时也能看清是哪个游戏）。 */
import { computed, ref, watch } from "vue";

const props = defineProps<{
  src?: string | null;
  fallbackSrc?: string | null;
  alt?: string | null;
}>();

/** 已失败的候选图数量：0 = 主图，1 = 备选，再失败就放弃 */
const failures = ref(0);

/** 去重后的候选链（海报和封面是同一个 URL 时只试一次） */
const candidates = computed(() => {
  const list: string[] = [];
  for (const url of [props.src, props.fallbackSrc]) {
    if (url && !list.includes(url)) list.push(url);
  }
  return list;
});

watch(candidates, () => {
  failures.value = 0;
});

const showSrc = computed(() => candidates.value[failures.value] ?? null);
</script>

<template>
  <img
    v-if="showSrc"
    :src="showSrc"
    :alt="props.alt ?? ''"
    loading="lazy"
    @error="failures += 1"
  />
  <div v-else class="fallback">
    <span>{{ props.alt ?? "无封面" }}</span>
  </div>
</template>
