<script setup lang="ts">
import { useToasts } from "../composables/toast";

const { toasts, dismissToast } = useToasts();

const ICONS: Record<string, string> = {
  ok: "✓",
  err: "✕",
  warn: "!",
  info: "i",
};
</script>

<template>
  <div class="toast-host">
    <TransitionGroup name="toast">
      <div v-for="toast in toasts" :key="toast.id" class="toast" :class="toast.kind">
        <strong
          :style="{
            color:
              toast.kind === 'ok'
                ? 'var(--ok)'
                : toast.kind === 'err'
                  ? 'var(--danger)'
                  : toast.kind === 'warn'
                    ? 'var(--warn)'
                    : 'var(--accent)',
          }"
          >{{ ICONS[toast.kind] ?? "·" }}</strong
        >
        <div class="msg">
          <strong>{{ toast.title }}</strong>
          <span v-if="toast.message">{{ toast.message }}</span>
        </div>
        <div class="spacer" />
        <button class="icon-btn" title="关闭" @click="dismissToast(toast.id)">×</button>
      </div>
    </TransitionGroup>
  </div>
</template>

<style scoped>
.toast-enter-active,
.toast-leave-active {
  transition: all 0.18s ease;
}

.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateX(16px);
}
</style>
