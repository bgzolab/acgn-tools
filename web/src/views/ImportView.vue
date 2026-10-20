<script setup lang="ts">
/** 导入与同步：Steam 账号管理、愿望单 / 库存、离线 JSON、元数据补全、历史记录。 */
import { computed, onMounted, ref } from "vue";

import {
  deleteAccount,
  enrich,
  importJson,
  importSteamLibrary,
  importSteamWishlist,
  listRuns,
  patchAccountCountry,
  resolveAccount,
  restoreJson,
  syncSteamAchievements,
} from "../api";
import type {
  AccountSummary,
  AchievementSyncSummary,
  EnrichSummary,
  ImportResponse,
  ImportRun,
} from "../api/types";
import AccountAvatar from "../components/AccountAvatar.vue";
import GameCard from "../components/GameCard.vue";
import { formatDate, formatDateTime, relativeTime } from "../composables/format";
import { errorText, pushToast } from "../composables/toast";
import { useAccountsStore } from "../stores/accounts";
import { useMetaStore } from "../stores/meta";

const meta = useMetaStore();
const accounts = useAccountsStore();

/** 数字输入框可能被清空、输入非法值或超长数字串：非正整数退回默认值，超上限截断 */
function positiveIntOr(value: number, fallback: number, max: number): number {
  if (!Number.isInteger(value) || value <= 0) return fallback;
  return Math.min(value, max);
}

// ---------------------------------------------------------------- 账号
/** 记在 localStorage 里：刷新页面后仍然停留在上次选的账号 */
const SELECTED_ACCOUNT_KEY = "acgn.selectedAccountId";
const selectedAccountId = ref<number | null>(null);
const selectedAccount = computed(() => accounts.byId(selectedAccountId.value));

const addDraft = ref("");
const adding = ref(false);
/** 区域编辑：点「改区域」进入行内输入 */
const countryEditingId = ref<number | null>(null);
const countryDraft = ref("");
const savingCountry = ref(false);
/** 删除账号的二段确认 */
const removeArmedId = ref<number | null>(null);
const removingAccount = ref(false);

/** Steam 资料的在线状态（personastate）→ 中文；其它来源 / 缺字段返回 null。 */
const PERSONA_STATE_LABELS: Record<number, string> = {
  0: "离线",
  1: "在线",
  2: "忙碌",
  3: "离开",
  4: "打盹",
  5: "想交易",
  6: "想玩游戏",
};

function personaState(account: AccountSummary): string | null {
  const value = account.metadata?.["personastate"];
  return typeof value === "number" ? (PERSONA_STATE_LABELS[value] ?? null) : null;
}

/** 资料里的 Unix 秒字段（timecreated / lastlogoff）→ ISO；没有则 null。 */
function profileTime(account: AccountSummary, key: string): string | null {
  const value = account.metadata?.[key];
  if (typeof value !== "number" || value <= 0) return null;
  return new Date(value * 1000).toISOString();
}

function selectAccount(id: number) {
  selectedAccountId.value = id;
  window.localStorage.setItem(SELECTED_ACCOUNT_KEY, String(id));
}

async function loadAccounts() {
  await accounts.load(true);
  const ids = new Set(accounts.items.map((account) => account.id));
  if (selectedAccountId.value !== null && ids.has(selectedAccountId.value)) return;

  const stored = Number(window.localStorage.getItem(SELECTED_ACCOUNT_KEY));
  if (Number.isFinite(stored) && ids.has(stored)) {
    selectedAccountId.value = stored;
    return;
  }
  const first = accounts.items[0];
  selectedAccountId.value = first ? first.id : null;
  if (first) window.localStorage.setItem(SELECTED_ACCOUNT_KEY, String(first.id));
}

/** 输入 ID → 自动建号（已存在则刷新资料）→ 选中 */
async function addAccount() {
  const input = addDraft.value.trim();
  if (!input) {
    pushToast("warn", "请先输入 SteamID", "支持 17 位 steamID64、个性域名或个人主页链接");
    return;
  }

  adding.value = true;
  try {
    const result = await resolveAccount(input);
    addDraft.value = "";
    await loadAccounts();
    selectAccount(result.account.id);
    const name = result.account.display_name?.trim() || result.account.external_id;
    const detail = result.warnings[0]
      ? result.warnings[0]
      : `${name} · ${result.account.external_id} · 区域 ${result.account.effective_country?.toUpperCase() ?? "未知"}`;
    pushToast(result.created ? "ok" : "warn", result.created ? "账号已添加" : "账号已存在，资料已刷新", detail);
  } catch (error) {
    const { title, message } = errorText(error);
    pushToast("err", title, message);
  } finally {
    adding.value = false;
  }
}

function startCountryEdit(account: AccountSummary) {
  countryEditingId.value = account.id;
  countryDraft.value = account.country_override ?? "";
}

function cancelCountryEdit() {
  countryEditingId.value = null;
  countryDraft.value = "";
}

async function saveCountry() {
  const id = countryEditingId.value;
  if (id === null) return;

  savingCountry.value = true;
  try {
    const account = await patchAccountCountry(id, countryDraft.value.trim());
    await accounts.load(true);
    cancelCountryEdit();
    pushToast(
      "ok",
      "价格区域已更新",
      account.effective_country
        ? `按 ${account.effective_country.toUpperCase()} 补价；旧区域的价格缓存已清空`
        : "已恢复跟随 Steam 资料区域",
    );
  } catch (error) {
    const { title, message } = errorText(error);
    pushToast("err", title, message);
  } finally {
    savingCountry.value = false;
  }
}

async function removeAccount(id: number) {
  if (removeArmedId.value !== id) {
    removeArmedId.value = id;
    return;
  }

  removingAccount.value = true;
  try {
    await deleteAccount(id);
    removeArmedId.value = null;
    await loadAccounts();
    pushToast("ok", "账号已删除", "关联已移除；游戏条目与个人状态保留");
  } catch (error) {
    const { title, message } = errorText(error);
    pushToast("err", title, message);
  } finally {
    removingAccount.value = false;
  }
}

// ---------------------------------------------------------------- Steam（愿望单 + 库存）
const includeDlc = ref(true);
const includeNonGames = ref(false);
const running = ref<{ kind: "wishlist" | "library"; dry: boolean } | null>(null);
const result = ref<ImportResponse | null>(null);
/** 本次操作针对的账号名（预览时 run 为 null，用它展示） */
const lastRunAccount = ref("");

/** 结果来自哪种导入：徽章按 mode 映射（JSON 导入 / 备份还原不能标成「愿望单」） */
const IMPORT_MODE_LABELS: Record<string, string> = {
  steam_wishlist: "愿望单",
  steam_library: "库存",
  json: "JSON 导入",
  restore: "备份还原",
  manual: "手动",
};

const resultModeLabel = computed(
  () => IMPORT_MODE_LABELS[result.value?.summary.mode ?? ""] ?? result.value?.summary.mode ?? "—",
);

/** 结果是不是库存导入：missing 文案不同（愿望单 vs 库存） */
const resultIsLibrary = computed(() => result.value?.summary.mode === "steam_library");

/** 本次结果对应的账号名（预览时 run 为 null） */
const resultAccountLabel = computed(() => {
  const fromRun = result.value?.run?.args?.account;
  if (typeof fromRun === "string" && fromRun) return fromRun;
  return lastRunAccount.value;
});

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
  const account = selectedAccount.value;
  if (!account) {
    pushToast("warn", "请先选择账号", "在上方添加账号，或点选要同步的账号卡片");
    return;
  }

  running.value = { kind, dry: dryRun };
  lastRunAccount.value = accounts.label(account);
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
    const body = { account_id: account.id, preview: !dryRun, options };
    const response =
      kind === "wishlist" ? await importSteamWishlist(body) : await importSteamLibrary(body);
    result.value = response;
    const summary = response.summary;
    pushToast(
      dryRun ? "warn" : "ok",
      `${accounts.label(account)} · ${label}${dryRun ? "预览完成（未写入）" : "导入完成"}`,
      `数据源 ${summary.source_mode} · 新增 ${summary.added} / 更新 ${summary.updated} / 未变 ${summary.unchanged} / 跳过 ${summary.skipped} · ${summary.duration_ms}ms`,
    );
    if (!dryRun) {
      // 条目数 / 最近同步时间会变；顺便让库页的账号筛选用到新数据
      await Promise.all([loadHistory(), accounts.load(true)]);
    }
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
  const account = selectedAccount.value;
  if (!account) {
    pushToast("warn", "请先选择账号", "成就是「某个账号的进度」，先在账号卡片里选一个");
    return;
  }

  achievementRunning.value = true;
  try {
    const summary = await syncSteamAchievements({
      account_id: account.id,
      // 输入框清空时不能发空字符串：后端 Option<i64> 会报英文 serde 400
      limit: positiveIntOr(achievementLimit.value, 200, 500),
    });
    achievementResult.value = summary;
    pushToast(
      summary.promoted > 0 ? "ok" : "warn",
      summary.promoted > 0 ? "成就检查完成" : "成就检查完成（没有新全成就）",
      `检查 ${summary.checked} · 更新 ${summary.updated}（含全成就升级 ${summary.promoted}）` +
        ` · 未变 ${summary.unchanged} · 该账号没有 ${summary.unavailable} · 失败 ${summary.failed}` +
        (summary.pending > 0 ? ` · 还剩 ${summary.pending} 个` : ""),
    );
    await Promise.all([loadHistory(), accounts.load(true)]);
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
    enrichResult.value = await enrich(positiveIntOr(enrichLimit.value, 20, 200));
    const summary = enrichResult.value;
    pushToast(
      summary.succeeded > 0 || summary.account_prices > 0 ? "ok" : "warn",
      "元数据补全结束",
      `详情 ${summary.succeeded}/${summary.requested} · 账号区域价格 ${summary.account_prices} 条 · 失败 ${summary.failed} · ${summary.duration_ms}ms`,
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
  void loadAccounts();
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

  <!-- Steam：账号 + 愿望单 / 库存 -->
  <div class="panel">
    <div class="panel-head">
      <div>
        <h2>① Steam 账号</h2>
        <div class="desc">
          输入一次 SteamID 就建档；之后直接选账号同步，不用再记 ID。
          昵称、头像、区域等资料会在每次同步时刷新（原始资料整份保存，方便以后补展示字段）。
        </div>
      </div>
      <span class="badge">共 {{ accounts.items.length }} 个账号</span>
    </div>

    <div class="row" style="margin-bottom: 6px">
      <input
        id="steam-id"
        v-model="addDraft"
        class="input"
        style="flex: 1; min-width: 260px"
        placeholder="76561198xxxxxxxxx / https://steamcommunity.com/id/xxx / 个性域名"
        @keyup.enter="addAccount"
      />
      <button class="btn primary" :disabled="adding" @click="addAccount">
        {{ adding ? "解析中…" : "添加账号" }}
      </button>
    </div>
    <div class="hint" style="margin-bottom: 14px">
      支持 17 位 steamID64、个性域名（如 bgzocn）或主页链接；已存在的账号只会刷新资料，不会重复建档
    </div>

    <div v-if="!accounts.items.length && !accounts.loading" class="banner">
      <span>ⓘ</span>
      <div>还没有账号：先输入 SteamID 添加一个，再同步它的愿望单 / 库存。</div>
    </div>
    <div v-if="accounts.error" class="banner danger" style="margin-bottom: 12px">
      <span>✕</span>
      <div>{{ accounts.error }}</div>
    </div>
    <div v-else class="account-grid">
      <article
        v-for="account in accounts.items"
        :key="account.id"
        class="account-card"
        :class="{ active: account.id === selectedAccountId }"
        @click="selectAccount(account.id)"
      >
        <AccountAvatar
          :src="account.avatar_url"
          :name="account.display_name?.trim() || account.external_id"
          :size="46"
        />
        <div class="account-main">
          <div class="account-name">
            {{ accounts.label(account) }}
            <span v-if="account.id === selectedAccountId" class="badge ok">已选择</span>
            <span v-if="account.country_override" class="badge warn">手动区域</span>
          </div>
          <div class="hint">{{ account.external_id }}</div>
          <div class="hint">
            库存 {{ account.owned_count }} · 愿望单 {{ account.wishlist_count }} · 最近同步
            {{ relativeTime(account.last_synced_at) }} · 添加于 {{ formatDate(account.created_at) }}
          </div>
          <div v-if="personaState(account) || profileTime(account, 'timecreated')" class="hint">
            <template v-if="personaState(account)">状态 {{ personaState(account) }}</template>
            <template v-if="profileTime(account, 'timecreated')">
              · 注册于 {{ formatDate(profileTime(account, "timecreated")) }}
            </template>
            <template v-if="profileTime(account, 'lastlogoff')">
              · 上次在线 {{ relativeTime(profileTime(account, "lastlogoff")) }}
            </template>
          </div>
          <div class="row tight" style="margin-top: 6px">
            <span
              class="badge"
              :title="account.country_code ? `Steam 资料区域：${account.country_code}` : 'Steam 资料未提供区域'"
            >
              区域 {{ account.effective_country?.toUpperCase() ?? "未知" }}
            </span>
            <button class="btn sm ghost" @click.stop="startCountryEdit(account)">改区域</button>
            <a
              v-if="account.profile_url"
              class="btn sm ghost"
              :href="account.profile_url"
              target="_blank"
              rel="noreferrer"
              @click.stop
              >Steam 主页 ↗</a
            >
            <button
              class="btn sm ghost danger-text"
              :disabled="removingAccount"
              @click.stop="removeAccount(account.id)"
            >
              {{ removeArmedId === account.id ? "确认删除账号" : "删除账号" }}
            </button>
            <button
              v-if="removeArmedId === account.id"
              class="btn sm ghost"
              @click.stop="removeArmedId = null"
            >
              取消
            </button>
          </div>
          <div v-if="countryEditingId === account.id" class="row tight" style="margin-top: 6px">
            <input
              v-model="countryDraft"
              class="input"
              style="width: 84px"
              maxlength="2"
              placeholder="us"
              @click.stop
            />
            <button class="btn sm primary" :disabled="savingCountry" @click.stop="saveCountry">
              保存
            </button>
            <button class="btn sm ghost" @click.stop="cancelCountryEdit">取消</button>
            <span class="hint">两位字母；留空跟随 Steam 资料</span>
          </div>
        </div>
      </article>
    </div>

    <div class="divider" />

    <div class="panel-head">
      <div>
        <h2 style="font-size: 16px">同步选中的账号</h2>
        <div class="desc">
          <template v-if="selectedAccount">
            当前：{{ accounts.label(selectedAccount) }}（{{ selectedAccount.external_id }}）
          </template>
          <template v-else>先在上方选择一个账号</template>
        </div>
      </div>
      <span class="badge">source_mode: {{ result?.summary.source_mode ?? "—" }}</span>
    </div>

    <div class="grid cols-2">
      <div>
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
            <button
              class="btn primary"
              :disabled="running !== null || !selectedAccount"
              @click="runSteam('wishlist', false)"
            >
              {{ isRunning("wishlist", false) ? "导入中…" : "导入愿望单" }}
            </button>
            <button
              class="btn"
              :disabled="running !== null || !selectedAccount"
              @click="runSteam('wishlist', true)"
            >
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
              :disabled="running !== null || !libraryReady || !selectedAccount"
              @click="runSteam('library', false)"
            >
              {{ isRunning("library", false) ? "同步中…" : "同步库存" }}
            </button>
            <button
              class="btn"
              :disabled="running !== null || !libraryReady || !selectedAccount"
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
            逐游戏检查成就进度（约 8 个/秒）；全成就的条目会标记为「已通关」，
            你手动标过的状态不会被回退。进度是「某个账号的」，按选中的账号检查
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
              :disabled="achievementRunning || !libraryReady || !selectedAccount"
              @click="runAchievements"
            >
              {{ achievementRunning ? "检查中…" : "同步成就" }}
            </button>
          </div>
          <div v-if="achievementResult" class="hint" style="margin-top: 6px">
            检查 {{ achievementResult.checked }} · 更新 {{ achievementResult.updated }}（含全成就升级
            {{ achievementResult.promoted }}） · 未变 {{ achievementResult.unchanged }} · 该账号没有
            {{ achievementResult.unavailable }} · 失败 {{ achievementResult.failed }}
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
            <li>每个游戏关联到它的账号：库存 / 愿望单按账号分别统计、分别筛选</li>
            <li>多个账号拥有同一个游戏时，时长取最大值，小号同步不会覆盖主号</li>
            <li>元数据（标题、封面、价格、评价）以 Steam 为准</li>
            <li>状态、评分、备注、进度以你本地为准，重复同步不会被打回去</li>
            <li>有游玩时长的已拥有游戏算「在玩」，没玩过的算「待玩」</li>
            <li>收成就检查会把全成就的游戏标为「已通关」，你手动标过的状态不会回退</li>
            <li>Steam 时长：没手填过就自动跟随；手填过则保留本地，清空可恢复跟随</li>
            <li>来源里消失的条目只按该账号提示、不删除</li>
            <li>价格为账号区域价格：与全局区域不同的账号会在「补全元数据」时单独取价</li>
          </ul>
        </div>
      </div>
    </div>

    <template v-if="result">
      <div class="divider" />
      <div class="row" style="margin-bottom: 10px">
        <strong v-if="result.run">运行 #{{ result.run.id }}</strong>
        <strong v-else>预览结果（未写入）</strong>
        <span v-if="resultAccountLabel" class="badge">{{ resultAccountLabel }}</span>
        <span class="badge">{{ resultModeLabel }}</span>
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
        上次：详情 {{ enrichResult.succeeded }} / 失败 {{ enrichResult.failed }} · 账号区域价格
        {{ enrichResult.account_prices }} 条 · 数据源 {{ enrichResult.source_mode }}
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

    <div v-if="history.length" class="table-scroll">
      <table class="table">
        <thead>
          <tr>
            <th>#</th>
            <th>时间</th>
            <th>类型</th>
            <th>账号</th>
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
              <td>{{ String(run.args?.account ?? "—") }}</td>
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
              <td colspan="13" class="wrap">
                <div v-if="run.error" class="banner danger" style="margin-bottom: 6px">{{ run.error }}</div>
                <ul v-if="run.warnings.length" class="warn-list">
                  <li v-for="(warning, index) in run.warnings" :key="index">{{ warning }}</li>
                </ul>
              </td>
            </tr>
          </template>
        </tbody>
      </table>
    </div>
    <div v-else class="empty">还没有导入记录</div>
  </div>
</template>
