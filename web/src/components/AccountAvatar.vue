<script setup lang="ts">
/** 账号头像：加载失败（或没有 URL）时用首字母占位，离线 demo 也不会出现破图。 */
import { computed, ref, watch } from "vue";

const props = defineProps<{
  src?: string | null;
  name?: string | null;
  size?: number;
}>();

const failed = ref(false);

// 切换账号 / 资料刷新后重新尝试加载
watch(
  () => props.src,
  () => {
    failed.value = false;
  },
);

const initial = computed(() => {
  const name = props.name?.trim();
  if (!name) return "?";
  // 中文名取第一个字，英文名取首字母
  return [...name][0]?.toUpperCase() ?? "?";
});

const sizePx = computed(() => `${props.size ?? 32}px`);
</script>

<template>
  <span class="avatar" :style="{ width: sizePx, height: sizePx }" :title="name ?? undefined">
    <img v-if="src && !failed" :src="src" :alt="name ?? '账号头像'" @error="failed = true" />
    <span v-else class="avatar-fallback">{{ initial }}</span>
  </span>
</template>
