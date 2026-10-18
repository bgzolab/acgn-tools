<script setup lang="ts">
/** 导入与同步：Steam 愿望单 / 库存、离线 JSON、元数据补全、历史记录。 */
import { computed, onMounted, ref } from "vue";

import { enrich, importJson, importSteamLibrary, importSteamWishlist, listRuns, restoreJson, syncSteamAchievements } from "../api";
import type { AchievementSyncSummary, EnrichSummary, ImportResponse, ImportRun } from "../api/types";
import GameCard from "../components/GameCard.vue";
import { formatDateTime, relativeTime } from "../composables/format";
import { errorText, pushToast } from "../composables/toast";
import { useMetaStore } from "../stores/meta";

const meta = useMetaStore();

// ---------------------------------------------------------------- Steam（愿望单 + 库存）
const steamId = ref("");
const includeDlc = ref(true);
const includeNonGames = ref(false);
const running = ref<{ kind: "wishlist" | "library"; dry: boolean } | null>(null);
const result = ref<ImportResponse | null>(null);

/** 结果是不是库存导入：missing 文案不同（愿望单 vs 库存） */
const resultIsLibrary = computed(() => result.value?.summary.mode === "steam_library");

/** 库存导入需要 API Key；fixture 离线模式不受此限制 */
const libraryReady = computed(() => {
  const steam = meta.config?.steam;
  if (!steam) return false;
  return steam.mode === "fixture" || steam.has_api_key;
});

function isRunning(kind: "wishlist" | "library", dry: boolean): boolean {
  return running.value?.kind === kind && running.value.dry === dry;
}

async function runSteam(kind: "wishlist" | "library", dryRun: boolean) {
  const input = steamId.value.trim();
  if (!input) {
    pushToast("warn", "请先填写 SteamID", "支持 17 位 steamID64、个性域名或个人主页链接");
    return;
  }

  running.value = { kind, dry: dryRun };
  const label = kind === "wishlist" ? "愿望单" : "库存";
  try {
    // 库存接口一次返回全库 + 时长，没有 DLC / 非游戏过滤；选项只传 dry_run
    const options =
      kind === "wishlist"
        ? {
            include_dlc: includeDlc.value,
            include_non_games: includeNonGames.value,
            dry_run: dryRun,
          }
        : { dry_run: dryRun };
    const body = { steam_id: input, preview: !dryRun, options };
    const response =
      kind === "wishlist" ? await importSteamWishlist(body) : await importSteamLibrary(body);
    result.value = response;
    const summary = response.summary;
    pushToast(
      dryRun ? "warn" : "ok",
      `${label}${dryRun ? "预览完成（未写入）" : "导入完成"}`,
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

// ---------------------------------------------------------------- 成就同步
const achievementLimit = ref(200);
const achievementRunning = ref(false);
const achievementResult = ref<AchievementSyncSummary | null>(null);

async function runAchievements() {
  const input = steamId.value.trim();
  if (!input) {
    pushToast("warn", "请先填写 SteamID", "成就是「某个账号的进度」，需要先填 SteamID");
    return;
  }

  achievementRunning.value = true;
  try {
    const summary = await syncSteamAchievements({
      steam_id: input,
      limit: achievementLimit.value,
    });
    achievementResult.value = summary;
    pushToast(
      summary.promoted > 0 ? "ok" : "warn",
      summary.promoted > 0 ? "成就检查完成" : "成就检查完成（没有新全成就）",
      `检查 ${summary.checked} · 全成就升级 ${summary.promoted} · 未变化 ${summary.unchanged} · 取不到 ${summary.failed}` +
        (summary.pending > 0 ? ` · 还剩 ${summary.pending} 个` : ""),
    );
    await loadHistory();
  } catch (error) {
    const { title, message } = errorText(error);
    pushToast("err", title, message);
  } finally {
    achievementRunning.value = false;
  }
}

// ---------------------------------------------------------------- 离线 JSON
const jsonDraft = ref("");
const jsonRunning = ref(false);
/** 仅预览：走 dry_run，不落库、不写运行记录 */
const jsonDryRun = ref(false);

/** 导出文件的元素是 `{game, entry}`；扁平条目（SourceItem）才是合并导入。 */
function looksLikeRestore(items: unknown[]): boolean {
  const first = items[0];
  return (
    typeof first === "object" && first !== null && "game" in (first as Record<string, unknown>)
  );
}

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

  // 自动识别：`{game, entry}` 是导出文件的还原结构，扁平的 SourceItem 走合并导入
  const restore = looksLikeRestore(items);

  jsonRunning.value = true;
  try {
    const response = restore
      ? await restoreJson({ items, dry_run: jsonDryRun.value, preview: !jsonDryRun.value })
      : await importJson({
          items,
          preview: !jsonDryRun.value,
          options: { dry_run: jsonDryRun.value },
        });

    if (!jsonDryRun.value) result.value = response;

    const summary = response.summary;
    const counts = `新增 ${summary.added} / 更新 ${summary.updated} / 未变 ${summary.unchanged} / 跳过 ${summary.skipped}`;
    pushToast(
      jsonDryRun.value ? "warn" : "ok",
      `${restore ? "还原" : "导入"}${jsonDryRun.value ? "预览完成（未写入）" : "完成"}`,
      counts,
    );
    if (!jsonDryRun.value) {
      jsonDraft.value = "";
      await loadHistory();
    }
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

  <!-- Steam：愿望单 + 库存 -->
  <div class="panel">
    <div class="panel-head">
      <div>
        <h2>① Steam 导入</h2>
        <div class="desc">
          不需要 cookie。愿望单先试公开接口（Steam 收紧匿名访问时自动回退官方接口）；
          库存走官方 GetOwnedGames 接口，会一并把游玩时长拉下来。
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
            @keyup.enter="runSteam('wishlist', false)"
          />
          <div class="hint" style="margin-top: 5px">
            支持 17 位 ID、个性域名（如 bgzocn）或主页链接；个性域名会自动解析成 64 位 ID
          </div>
        </div>

        <div class="divider" />

        <div class="field">
          <span class="label">愿望单</span>
          <div class="row tight" style="margin-bottom: 8px">
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
            <button class="btn primary" :disabled="running !== null" @click="runSteam('wishlist', false)">
              {{ isRunning("wishlist", false) ? "导入中…" : "导入愿望单" }}
            </button>
            <button class="btn" :disabled="running !== null" @click="runSteam('wishlist', true)">
              {{ isRunning("wishlist", true) ? "预览中…" : "仅预览" }}
            </button>
          </div>
        </div>

        <div class="divider" />

        <div class="field">
          <span class="label">库存（含游玩时长）</span>
          <div class="hint" style="margin-bottom: 8px">
            需要「游戏详情」公开 + 后端配置 STEAM_API_KEY；有游玩时长的条目算「在玩」，
            没玩过的算「待玩」，Steam 时长在你没有手填过时自动跟随
          </div>
          <div class="row">
            <button
              class="btn primary"
              :disabled="running !== null || !libraryReady"
              @click="runSteam('library', false)"
            >
              {{ isRunning("library", false) ? "同步中…" : "同步库存" }}
            </button>
            <button
              class="btn"
              :disabled="running !== null || !libraryReady"
              @click="runSteam('library', true)"
            >
              {{ isRunning("library", true) ? "预览中…" : "仅预览" }}
            </button>
          </div>
          <div v-if="!libraryReady" class="hint" style="margin-top: 6px">
            ⚠ 未检测到 STEAM_API_KEY：愿望单导入不受影响；库存接口不接受匿名调用
          </div>
        </div>

        <div class="divider" />

        <div class="field">
          <span class="label">成就检查（全成就 → 已通关）</span>
          <div class="hint" style="margin-bottom: 8px">
            逐游戏检查成就进度（约 10 个/秒）；全成就的条目会标记为「已通关」，
            你手动标过的状态不会被回退。进度是「某个账号的」，使用上面的 SteamID
          </div>
          <div class="row">
            <input
              v-model.number="achievementLimit"
              class="input"
              style="width: 90px"
              type="number"
              min="1"
              max="500"
            />
            <span class="hint">个 / 次</span>
            <button
              class="btn"
              :disabled="achievementRunning || !libraryReady"
              @click="runAchievements"
            >
              {{ achievementRunning ? "检查中…" : "同步成就" }}
            </button>
          </div>
          <div v-if="achievementResult" class="hint" style="margin-top: 6px">
            检查 {{ achievementResult.checked }} · 全成就升级 {{ achievementResult.promoted }} ·
            未变化 {{ achievementResult.unchanged }} · 取不到 {{ achievementResult.failed }}
            <template v-if="achievementResult.pending > 0">
              · 还剩 {{ achievementResult.pending }} 个，再点一次继续
            </template>
          </div>
          <ul v-if="achievementResult?.warnings.length" class="warn-list" style="margin-top: 6px">
            <li v-for="(warning, index) in achievementResult.warnings.slice(0, 5)" :key="index">
              {{ warning }}
            </li>
          </ul>
        </div>
      </div>

      <div class="banner">
        <span>ⓘ</span>
        <div>
          <strong>同步语义</strong>
          <ul class="warn-list">
            <li>元数据（标题、封面、价格、评价）以 Steam 为准</li>
            <li>状态、评分、备注、进度以你本地为准，重复同步不会被打回去</li>
            <li>有游玩时长的已拥有游戏算「在玩」，没玩过的算「待玩」</li>
            <li>成就检查会把全成就的游戏标为「已通关」，你手动标过的状态不会回退</li>
            <li>Steam 时长：没手填过就自动跟随；手填过则保留本地，清空可恢复跟随</li>
            <li>来源里消失的条目只提示、不删除</li>
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
        <span class="badge">{{ resultIsLibrary ? "库存" : "愿望单" }}</span>
        <span class="badge ok">新增 {{ result.summary.added }}</span>
        <span class="badge">更新 {{ result.summary.updated }}</span>
        <span class="badge">未变 {{ result.summary.unchanged }}</span>
        <span class="badge">跳过 {{ result.summary.skipped }}</span>
        <span v-if="result.summary.missing" class="badge warn">
          {{ resultIsLibrary ? "已不在库存" : "已不在愿望单" }} {{ result.summary.missing }}
        </span>
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
          粘贴 <strong>导出的整份 JSON</strong> 即做备份还原（含状态 / 评分 / 备注，文件优先）；
          粘贴扁平条目（含 <code>external_id</code>）则走合并导入
        </div>
      </div>
      <button class="btn sm ghost" @click="jsonDraft = JSON_SAMPLE">填入示例</button>
    </div>
    <textarea
      v-model="jsonDraft"
      class="textarea"
      style="min-height: 132px"
      placeholder='粘贴导出文件，或 [ { "external_id": "xxx", "title": "示例条目", "status": "wishlist" } ]'
    />
    <div class="row" style="margin-top: 10px">
      <button class="btn" :disabled="jsonRunning" @click="runJsonImport">
        {{ jsonRunning ? "处理中…" : jsonDryRun ? "仅预览" : "导入 / 还原" }}
      </button>
      <label class="switch">
        <input v-model="jsonDryRun" type="checkbox" />
        仅预览（不写入）
      </label>
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
