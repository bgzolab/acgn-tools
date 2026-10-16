<script setup lang="ts">
/** 导入与同步：Steam 愿望单、离线 JSON、元数据补全、历史记录。 */
import { onMounted, ref } from "vue";

import { enrich, importJson, importSteamWishlist, listRuns } from "../api";
import type { EnrichSummary, ImportResponse, ImportRun } from "../api/types";
import GameCard from "../components/GameCard.vue";
import { formatDateTime, relativeTime } from "../composables/format";
import { errorText, pushToast } from "../composables/toast";
import { useMetaStore } from "../stores/meta";

const meta = useMetaStore();

// ---------------------------------------------------------------- Steam 愿望单
const steamId = ref("");
const includeDlc = ref(true);
const includeNonGames = ref(false);
const running = ref<"import" | "dry" | null>(null);
const result = ref<ImportResponse | null>(null);

async function runSteam(dryRun: boolean) {
  const input = steamId.value.trim();
  if (!input) {
    pushToast("warn", "请先填写 SteamID", "支持 17 位 steamID64、个性域名或个人主页链接");
    return;
  }

  running.value = dryRun ? "dry" : "import";
  try {
    const response = await importSteamWishlist({
      steam_id: input,
      preview: !dryRun,
      options: {
        include_dlc: includeDlc.value,
        include_non_games: includeNonGames.value,
        dry_run: dryRun,
      },
    });
    result.value = response;
    const summary = response.summary;
    pushToast(
      dryRun ? "warn" : "ok",
      dryRun ? "预览完成（未写入）" : "导入完成",
      `数据源 ${summary.source_mode} · 新增 ${summary.added} / 更新 ${summary.updated} / 未变 ${summary.unchanged} / 跳过 ${summary.skipped} · ${summary.duration_ms}ms`,
    );
    if (!dryRun) await loadHistory();
  } catch (error) {
    const { title, message } = errorText(error);
    pushToast("err", title, message);
  } finally {
    running.value = null;
  }
}

// ---------------------------------------------------------------- 离线 JSON
const jsonDraft = ref("");
const jsonRunning = ref(false);

const JSON_SAMPLE = `{
  "items": [
    {
      "source": "manual",
      "external_id": "note-001",
      "title": "还没上 Steam 的想玩游戏",
      "status": "wishlist",
      "tags": ["独立", "解谜"],
      "release_raw": "2026 年待定"
    }
  ]
}`;

async function runJsonImport() {
  const raw = jsonDraft.value.trim();
  if (!raw) {
    pushToast("warn", "粘贴一段 JSON 再导入", `格式示例：${JSON_SAMPLE.slice(0, 40)}…`);
    return;
  }

  let items: unknown[];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (Array.isArray(parsed)) {
      items = parsed;
    } else if (parsed && typeof parsed === "object" && Array.isArray((parsed as { items?: unknown[] }).items)) {
      items = (parsed as { items: unknown[] }).items;
    } else if (parsed && typeof parsed === "object" && Array.isArray((parsed as { data?: unknown[] }).data)) {
      items = (parsed as { data: unknown[] }).data;
    } else {
      throw new Error("既不是数组，也没有 items / data 字段");
    }
  } catch (error) {
    pushToast("err", "JSON 解析失败", error instanceof Error ? error.message : String(error));
    return;
  }

  jsonRunning.value = true;
  try {
    const response = await importJson({ items, preview: true });
    result.value = response;
    pushToast(
      "ok",
      "JSON 导入完成",
      `新增 ${response.summary.added} / 更新 ${response.summary.updated} / 未变 ${response.summary.unchanged}`,
    );
    jsonDraft.value = "";
    await loadHistory();
  } catch (error) {
    const { title, message } = errorText(error);
    pushToast("err", title, message);
  } finally {
    jsonRunning.value = false;
  }
}

// ---------------------------------------------------------------- 元数据补全
const enrichLimit = ref(20);
const enriching = ref(false);
const enrichResult = ref<EnrichSummary | null>(null);

async function runEnrich() {
  enriching.value = true;
  try {
    enrichResult.value = await enrich(enrichLimit.value);
    const summary = enrichResult.value;
    pushToast(
      summary.succeeded > 0 ? "ok" : "warn",
      "元数据补全结束",
      `请求 ${summary.requested} · 成功 ${summary.succeeded} · 失败 ${summary.failed} · ${summary.duration_ms}ms`,
    );
    await loadHistory();
  } catch (error) {
    const { title, message } = errorText(error);
    pushToast("err", title, message);
  } finally {
    enriching.value = false;
  }
}

// ---------------------------------------------------------------- 历史
const history = ref<ImportRun[]>([]);
const expanded = ref<number | null>(null);

async function loadHistory() {
  try {
    history.value = await listRuns(20);
  } catch (error) {
    const { title, message } = errorText(error);
    pushToast("err", title, message);
  }
}

onMounted(() => {
  void meta.load();
  void loadHistory();
});
</script>

<template>
  <div class="page-head">
    <div>
      <h1>导入与同步</h1>
      <p class="sub">
        当前数据源：{{ meta.steamModeLabel }}
        <template v-if="meta.config?.steam.fixture"> · fixture：{{ meta.config.steam.fixture }}</template>
      </p>
    </div>
  </div>

  <!-- Steam 愿望单 -->
  <div class="panel">
    <div class="panel-head">
      <div>
        <h2>① 导入 Steam 愿望单</h2>
        <div class="desc">
          走公开接口，不需要 API Key；前提是 Steam「个人资料」「游戏详情」「愿望单」都是公开的
        </div>
      </div>
      <span class="badge">source_mode: {{ result?.summary.source_mode ?? "—" }}</span>
    </div>

    <div class="grid cols-2">
      <div>
        <div class="field">
          <label class="label" for="steam-id">SteamID64 / 个性域名 / 主页链接</label>
          <input
            id="steam-id"
            v-model="steamId"
            class="input"
            placeholder="76561198xxxxxxxxx 或 https://steamcommunity.com/id/xxx"
            @keyup.enter="runSteam(false)"
          />
          <div class="hint" style="margin-top: 5px">
            在 Steam 客户端「个人资料 → 编辑资料」或 steamid.io 可以查到 17 位 ID
          </div>
        </div>

        <div class="row tight" style="margin-bottom: 12px">
          <label class="switch">
            <input v-model="includeDlc" type="checkbox" />
            收下 DLC
          </label>
          <label class="switch">
            <input v-model="includeNonGames" type="checkbox" />
            收下软件/试玩
          </label>
        </div>

        <div class="row">
          <button class="btn primary" :disabled="running !== null" @click="runSteam(false)">
            {{ running === "import" ? "导入中…" : "开始导入" }}
          </button>
          <button class="btn" :disabled="running !== null" @click="runSteam(true)">
            {{ running === "dry" ? "预览中…" : "仅预览（不写入）" }}
          </button>
        </div>
      </div>

      <div class="banner">
        <span>ⓘ</span>
        <div>
          <strong>同步语义</strong>
          <ul class="warn-list">
            <li>元数据（标题、封面、价格、评价）以 Steam 为准</li>
            <li>状态、评分、备注、进度以你本地为准，重复同步不会被打回去</li>
            <li>已经从愿望单消失的条目只提示、不删除</li>
            <li>首次导入只拿到列表级信息，价格/简介等要点「补全元数据」</li>
          </ul>
        </div>
      </div>
    </div>

    <template v-if="result">
      <div class="divider" />
      <div class="row" style="margin-bottom: 10px">
        <strong v-if="result.run">运行 #{{ result.run.id }}</strong>
        <strong v-else>预览结果（未写入）</strong>
        <span class="badge ok">新增 {{ result.summary.added }}</span>
        <span class="badge">更新 {{ result.summary.updated }}</span>
        <span class="badge">未变 {{ result.summary.unchanged }}</span>
        <span class="badge">跳过 {{ result.summary.skipped }}</span>
        <span v-if="result.summary.missing" class="badge warn">已不在愿望单 {{ result.summary.missing }}</span>
        <span class="hint">
          {{ relativeTime(result.run?.started_at ?? new Date().toISOString()) }} ·
          {{ result.summary.duration_ms }}ms
        </span>
      </div>

      <ul v-if="result.summary.warnings.length" class="warn-list">
        <li v-for="(warning, index) in result.summary.warnings" :key="index">{{ warning }}</li>
      </ul>

      <div v-if="result.preview.length" class="game-grid" style="margin-top: 12px">
        <GameCard v-for="item in result.preview" :key="item.game.id" :item="item" />
      </div>
    </template>
  </div>

  <!-- 元数据补全 -->
  <div class="panel">
    <div class="panel-head">
      <div>
        <h2>② 补全元数据</h2>
        <div class="desc">
          对还没抓过商店详情的条目调用 Steam appdetails（有节流，一次别抓太多）
        </div>
      </div>
    </div>
    <div class="row">
      <input v-model.number="enrichLimit" class="input" style="width: 90px" type="number" min="1" max="200" />
      <span class="hint">条 / 次</span>
      <button class="btn" :disabled="enriching" @click="runEnrich">
        {{ enriching ? "抓取中…" : "开始补全" }}
      </button>
      <span v-if="enrichResult" class="hint">
        上次：成功 {{ enrichResult.succeeded }} / 失败 {{ enrichResult.failed }} · 数据源
        {{ enrichResult.source_mode }}
      </span>
    </div>
    <ul v-if="enrichResult?.warnings.length" class="warn-list">
      <li v-for="(warning, index) in enrichResult.warnings.slice(0, 6)" :key="index">{{ warning }}</li>
    </ul>
  </div>

  <!-- 离线 JSON -->
  <div class="panel">
    <div class="panel-head">
      <div>
        <h2>③ 离线 / 手工导入</h2>
        <div class="desc">
          粘贴 JSON 数组（或 <code>{ "items": [...] }</code>）导入；字段名与导出文件一致，可作为离线兜底
        </div>
      </div>
      <button class="btn sm ghost" @click="jsonDraft = JSON_SAMPLE">填入示例</button>
    </div>
    <textarea
      v-model="jsonDraft"
      class="textarea"
      style="min-height: 132px"
      placeholder='[ { "external_id": "xxx", "title": "示例条目", "status": "wishlist" } ]'
    />
    <div class="row" style="margin-top: 10px">
      <button class="btn" :disabled="jsonRunning" @click="runJsonImport">
        {{ jsonRunning ? "导入中…" : "导入这段 JSON" }}
      </button>
      <a class="btn ghost" :href="'/api/export/json'" target="_blank" rel="noreferrer">导出全量 JSON ↗</a>
    </div>
  </div>

  <!-- 历史 -->
  <div class="panel">
    <div class="panel-head">
      <div>
        <h2>④ 同步历史</h2>
        <div class="desc">每次导入 / 补全都会留档，失败也会记录原因</div>
      </div>
      <button class="btn sm ghost" @click="loadHistory">刷新</button>
    </div>

    <table v-if="history.length" class="table">
      <thead>
        <tr>
          <th>#</th>
          <th>时间</th>
          <th>类型</th>
          <th>来源</th>
          <th>新增</th>
          <th>更新</th>
          <th>未变</th>
          <th>跳过</th>
          <th>消失</th>
          <th>耗时</th>
          <th>状态</th>
          <th />
        </tr>
      </thead>
      <tbody>
        <template v-for="run in history" :key="run.id">
          <tr>
            <td>{{ run.id }}</td>
            <td :title="formatDateTime(run.started_at)">{{ relativeTime(run.started_at) }}</td>
            <td>{{ run.mode }}</td>
            <td>{{ String(run.args?.source_mode ?? "—") }}</td>
            <td>{{ run.added }}</td>
            <td>{{ run.updated }}</td>
            <td>{{ run.unchanged }}</td>
            <td>{{ run.skipped }}</td>
            <td>{{ run.missing }}</td>
            <td>{{ run.duration_ms ?? "—" }}ms</td>
            <td>
              <span
                class="badge"
                :class="run.status === 'ok' ? 'ok' : run.status === 'failed' ? 'danger' : 'warn'"
                >{{ run.status }}</span
              >
            </td>
            <td>
              <button
                v-if="run.warnings.length || run.error"
                class="btn sm ghost"
                @click="expanded = expanded === run.id ? null : run.id"
              >
                {{ expanded === run.id ? "收起" : `详情 (${run.warnings.length})` }}
              </button>
            </td>
          </tr>
          <tr v-if="expanded === run.id">
            <td colspan="12" class="wrap">
              <div v-if="run.error" class="banner danger" style="margin-bottom: 6px">{{ run.error }}</div>
              <ul v-if="run.warnings.length" class="warn-list">
                <li v-for="(warning, index) in run.warnings" :key="index">{{ warning }}</li>
              </ul>
            </td>
          </tr>
        </template>
      </tbody>
    </table>
    <div v-else class="empty">还没有导入记录</div>
  </div>
</template>
