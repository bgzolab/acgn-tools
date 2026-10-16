/** 后端配置 / 状态枚举（全局只拉一次）。 */

import { defineStore } from "pinia";
import { computed, ref } from "vue";

import { getConfig, getHealth } from "../api";
import { STATUS_LABELS, STATUS_ORDER, type PublicConfig, type Status } from "../api/types";

export const useMetaStore = defineStore("meta", () => {
  const config = ref<PublicConfig | null>(null);
  const health = ref<{ status: string; version: string; uptime_seconds: number; steam_mode: string } | null>(null);
  const loading = ref(false);
  const error = ref("");

  const statuses = computed<{ key: Status; label: string }[]>(() => {
    if (config.value?.statuses?.length) return config.value.statuses;
    return STATUS_ORDER.map((key) => ({ key, label: STATUS_LABELS[key] }));
  });

  const steamMode = computed(() => config.value?.steam.mode ?? "unknown");

  const steamModeLabel = computed(() => {
    switch (steamMode.value) {
      case "fixture":
        return "离线样例（fixture）";
      case "live_with_key":
        return "线上 + API Key";
      case "live":
        return "线上（公开接口）";
      default:
        return steamMode.value;
    }
  });

  async function load(force = false) {
    if (loading.value) return;
    if (config.value && !force) return;
    loading.value = true;
    error.value = "";
    try {
      const [nextConfig, nextHealth] = await Promise.all([getConfig(), getHealth()]);
      config.value = nextConfig;
      health.value = nextHealth;
    } catch (err) {
      error.value = err instanceof Error ? err.message : String(err);
    } finally {
      loading.value = false;
    }
  }

  return { config, health, loading, error, statuses, steamMode, steamModeLabel, load };
});
