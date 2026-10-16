<script setup lang="ts">
/** 设置：后端 / 数据源状态、导出、接口导航、路线图。 */
import { onMounted, ref } from "vue";

import { exportUrl, getHealth } from "../api";
import { relativeTime } from "../composables/format";
import { errorText, pushToast } from "../composables/toast";
import { useMetaStore } from "../stores/meta";

const meta = useMetaStore();
const reloading = ref(false);
const health = ref<{ version: string; uptime_seconds: number } | null>(null);

async function reload() {
  reloading.value = true;
  try {
    await meta.load(true);
    health.value = await getHealth();
    pushToast("ok", "已重新读取后端配置");
  } catch (error) {
    const { title, message } = errorText(error);
    pushToast("err", title, message);
  } finally {
    reloading.value = false;
  }
}

onMounted(async () => {
  await meta.load();
  try {
    health.value = await getHealth();
  } catch {
    /* 静默：健康检查失败会在看板/列表上直接暴露 */
  }
});
</script>

<template>
  <div class="page-head">
    <div>
      <h1>设置</h1>
      <p class="sub">这里只读展示后端状态；改配置请改环境变量后重启 acgn-server</p>
    </div>
    <button class="btn" :disabled="reloading" @click="reload">
      {{ reloading ? "读取中…" : "重新读取" }}
    </button>
  </div>

  <div class="grid cols-2">
    <div class="panel">
      <div class="panel-head"><h2>后端 / 数据源</h2></div>
      <dl class="kv">
        <dt>应用</dt>
        <dd>{{ meta.config?.app ?? "acgn-tools" }} v{{ meta.config?.version ?? "—" }}</dd>

        <dt>Schema</dt>
        <dd>rev {{ meta.config?.schema_revision ?? "—" }}</dd>

        <dt>Steam 模式</dt>
        <dd>{{ meta.steamModeLabel }}</dd>

        <dt>API Key</dt>
        <dd>{{ meta.config?.steam.has_api_key ? "已配置" : "未配置（走公开接口）" }}</dd>

        <dt>商店国家</dt>
        <dd>{{ meta.config?.steam.country ?? "—" }} / {{ meta.config?.steam.language ?? "—" }}</dd>

        <dt>fixture</dt>
        <dd>{{ meta.config?.steam.fixture ?? "未启用（走真实网络）" }}</dd>

        <dt>运行时长</dt>
        <dd>{{ health ? `${Math.floor(health.uptime_seconds / 60)} 分钟` : "—" }}</dd>
      </dl>

      <div class="divider" />
      <h3>常用环境变量</h3>
      <ul class="warn-list">
        <li><code>ACGN_BIND</code> 监听地址（默认 127.0.0.1:8787，要局域网访问改成 0.0.0.0:8787）</li>
        <li><code>ACGN_DB</code> SQLite 路径（默认 ./data/acgn.db）</li>
        <li><code>ACGN_WEB_DIR</code> 前端构建产物目录（默认 ./web/dist）</li>
        <li><code>STEAM_API_KEY</code> 可选，公开接口不可用时回退官方接口</li>
        <li><code>STEAM_COUNTRY</code> / <code>STEAM_LANGUAGE</code> 价格币种与商店语言</li>
        <li><code>ACGN_STEAM_FIXTURE</code> 指向 fixtures/steam 即进入离线模式</li>
      </ul>
    </div>

    <div class="panel">
      <div class="panel-head"><h2>数据</h2></div>
      <div class="row">
        <a class="btn" :href="exportUrl" target="_blank" rel="noreferrer">导出全量 JSON ↗</a>
        <a class="btn ghost" href="/api" target="_blank" rel="noreferrer">查看 API 列表 ↗</a>
      </div>
      <p class="hint" style="margin-top: 10px">
        导出的 JSON 可以直接通过「导入与同步 → 离线 / 手工导入」再灌回来，用于备份迁移。
      </p>

      <div class="divider" />
      <div class="panel-head"><h2>状态里都代表什么</h2></div>
      <ul class="warn-list">
        <li v-for="status in meta.statuses" :key="status.key">
          <code>{{ status.key }}</code> — {{ status.label }}
        </li>
      </ul>

      <div class="divider" />
      <div class="panel-head"><h2>路线图</h2></div>
      <ul class="warn-list">
        <li>接入 Bangumi：动画 / 漫画 / 小说条目 + 收藏进度双向同步</li>
        <li>接入小黑盒社区数据（无公开 API，需要专门调研 / 手动导入兜底）</li>
        <li>Tauri 桌面端：复用同一套前端与 core/store/steam 三个 crate</li>
        <li>多端同步：基于 schema_revision 与 import_runs 的增量合并</li>
      </ul>
      <p class="hint" style="margin-top: 10px">
        当前后端启动于 {{ relativeTime(new Date(Date.now() - (health?.uptime_seconds ?? 0) * 1000).toISOString()) }}
      </p>
    </div>
  </div>
</template>
