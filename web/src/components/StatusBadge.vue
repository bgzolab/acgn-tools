<script setup lang="ts">
import { computed } from "vue";

import { statusLabel } from "../composables/format";
import type { Status } from "../api/types";

const props = defineProps<{ status: Status; compact?: boolean }>();
const label = computed(() => statusLabel(props.status));
</script>

<template>
  <span class="badge" :data-status="status" :title="label">{{ label }}</span>
</template>
