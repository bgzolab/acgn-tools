<script setup lang="ts">
const props = defineProps<{
  page: number;
  totalPages: number;
  total: number;
  perPage: number;
}>();
const emit = defineEmits<{ go: [page: number] }>();
</script>

<template>
  <div class="row" style="justify-content: center; margin-top: 18px">
    <button class="btn sm" :disabled="page <= 1" @click="emit('go', page - 1)">← 上一页</button>
    <span class="hint">
      第 {{ page }} / {{ Math.max(totalPages, 1) }} 页 · 共 {{ total }} 条 · 每页 {{ perPage }}
    </span>
    <button class="btn sm" :disabled="page >= totalPages" @click="emit('go', page + 1)">
      下一页 →
    </button>
  </div>
</template>
