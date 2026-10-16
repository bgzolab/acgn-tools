<script setup lang="ts">
/** 图片 + 兜底占位：Steam CDN 被墙时也能看清是哪个游戏。 */
import { computed, ref, watch } from "vue";

const props = defineProps<{ src?: string | null; alt?: string | null }>();
const failed = ref(false);

watch(
  () => props.src,
  () => {
    failed.value = false;
  },
);

const show = computed(() => Boolean(props.src) && !failed.value);
</script>

<template>
  <img
    v-if="show"
    :src="props.src ?? ''"
    :alt="props.alt ?? ''"
    loading="lazy"
    @error="failed = true"
  />
  <div v-else class="fallback">
    <span>{{ props.alt ?? "无封面" }}</span>
  </div>
</template>
