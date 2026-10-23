<script setup lang="ts">
/** 应用外壳：侧边导航 + 内容区 + toast。 */
import { computed, onMounted } from "vue";
import { RouterLink, RouterView, useRoute } from "vue-router";

import ToastHost from "./components/ToastHost.vue";
import { useMetaStore } from "./stores/meta";

const route = useRoute();
const meta = useMetaStore();

const navItems = [
  { to: "/", icon: "◎", label: "看板", name: "dashboard" },
  { to: "/library", icon: "▤", label: "游戏库", name: "library" },
  { to: "/news", icon: "📰", label: "游戏新闻", name: "news" },
  { to: "/import", icon: "⇩", label: "导入与同步", name: "import" },
  { to: "/settings", icon: "⚙", label: "设置", name: "settings" },
];

const currentTitle = computed(() => String(route.meta.title ?? "看板"));

onMounted(() => {
  void meta.load();
});
</script>

<template>
  <div class="shell">
    <aside class="sidebar">
      <div class="brand">
        <div class="brand-logo">🎮</div>
        <div>
          <div class="brand-name">ACGN Tools</div>
          <div class="brand-sub">游戏库 · {{ currentTitle }}</div>
        </div>
      </div>

      <nav class="nav">
        <RouterLink
          v-for="item in navItems"
          :key="item.to"
          :to="item.to"
          class="nav-item"
          :class="{ active: route.name === item.name }"
        >
          <span class="nav-icon">{{ item.icon }}</span>
          <span>{{ item.label }}</span>
        </RouterLink>
      </nav>

      <div class="sidebar-footer">
        <div class="row tight">
          <span class="badge" :class="meta.steamMode === 'fixture' ? 'warn' : 'ok'">
            {{ meta.steamModeLabel }}
          </span>
        </div>
        <div v-if="meta.config">v{{ meta.config.version }} · schema rev {{ meta.config.schema_revision }}</div>
        <div v-if="meta.error" style="color: var(--danger)">后端未连接</div>
      </div>
    </aside>

    <main class="main">
      <RouterView />
    </main>

    <ToastHost />
  </div>
</template>
