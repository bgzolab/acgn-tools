#!/usr/bin/env node
/**
 * 生成 Steam 离线 fixture（给开发 / 演示 / 回归测试用）。
 *
 * 数据来源（公开接口，不需要 API Key）：
 *   - appreviews  → 好评档位 / 好评数（真实）
 *   - appdetails  → 中英文名 / 价格 / 简介 / 发售日 / 平台（真实）
 *   - library.json 的累计时长是脚本**确定性生成**的演示数据（GetOwnedGames 需要
 *     API Key 与真实账号，仓库里不能放任何人的真实库存），名字来自 appdetails。
 *
 * 用法：
 *   node scripts/gen-steam-fixture.mjs                 # 生成到 fixtures/steam/
 *   node scripts/gen-steam-fixture.mjs --library-only  # 只用现有 wishlist.json 生成 library.json（不联网）
 *   node scripts/gen-steam-fixture.mjs --out /tmp/steam
 *
 * 注意：除 `--library-only` 外脚本会真的访问 Steam，请确保网络通畅（国内需要走代理）。
 */

import { writeFile, readFile, mkdir } from "node:fs/promises";
import path from "node:path";

const APPIDS = [
  1245620, // ELDEN RING
  1086940, // Baldur's Gate 3
  1091500, // Cyberpunk 2077
  2358720, // Black Myth: Wukong
  413150, // Stardew Valley
  1145360, // Hades
  367520, // Hollow Knight
  1030300, // Hollow Knight: Silksong
  427520, // Factorio
  2246340, // Monster Hunter Wilds
  2679460, // Metaphor: ReFantazio
  2379780, // Balatro
  526870, // Satisfactory
  294100, // RimWorld
  548430, // Deep Rock Galactic
  105600, // Terraria
  881100, // Noita
  1794680, // Vampire Survivors
  1817230, // Hi-Fi RUSH
  814380, // Sekiro: Shadows Die Twice
  1888160, // ARMORED CORE VI: FIRES OF RUBICON
  1687950, // Persona 5 Royal
  975370, // Dwarf Fortress
  646570, // Slay the Spire
  949230, // Cities: Skylines II
  2694490, // Path of Exile 2
  1627720, // Lies of P
  2344520, // Diablo IV
];

const args = process.argv.slice(2);
const outIndex = args.indexOf("--out");
const OUT_DIR =
  outIndex >= 0 && args[outIndex + 1]
    ? path.resolve(args[outIndex + 1])
    : path.resolve(import.meta.dirname, "..", "fixtures", "steam");
/** 只用现有 wishlist.json 生成 library.json：不联网，适合日常调整库演示数据 */
const LIBRARY_ONLY = args.includes("--library-only");

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** 演示用累计时长（分钟）：确定性生成 —— 前几条像在玩，其余多为未玩。 */
function demoPlaytime(index) {
  return index % 5 === 0 ? 0 : (index * 173) % 6000;
}

/**
 * 从 wishlist 生成 GetOwnedGames 形态的 library.json。
 * 时长是演示数据；条目标记「已拥有」，导入后会落到「待玩」。
 */
function buildLibraryFile(wishlist) {
  const now = Math.floor(Date.now() / 1000);
  const games = Object.entries(wishlist)
    .filter(([, entry]) => entry?.name)
    .map(([appid, entry], index) => {
      const playtime = demoPlaytime(index);
      return {
        appid: Number(appid),
        name: entry.name,
        playtime_forever: playtime,
        playtime_2weeks: 0,
        ...(playtime > 0
          ? { rtime_last_played: now - (index + 1) * 7 * 24 * 3600 }
          : {}),
      };
    });

  return {
    steam_id: "76561198000000000",
    fetched_at: new Date().toISOString(),
    note: "样例数据：时长由 scripts/gen-steam-fixture.mjs 确定性生成（演示用，不是真实账号数据）",
    response: { game_count: games.length, games },
  };
}

async function writeLibraryFile(wishlist) {
  const libraryFile = buildLibraryFile(wishlist);
  await writeFile(
    path.join(OUT_DIR, "library.json"),
    `${JSON.stringify(libraryFile, null, 2)}\n`,
    "utf8",
  );
  console.log(`✔ library.json：${libraryFile.response.game_count} 条（演示时长）`);
}

async function getJson(url, attempt = 1) {
  try {
    const response = await fetch(url, {
      headers: {
        accept: "application/json",
        "accept-language": "en",
        "user-agent": "acgn-tools fixture generator",
      },
    });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return await response.json();
  } catch (error) {
    if (attempt >= 4) throw error;
    await sleep(1500 * attempt);
    return getJson(url, attempt + 1);
  }
}

async function appReviews(appid) {
  const data = await getJson(
    `https://store.steampowered.com/appreviews/${appid}?json=1&language=all&num_per_page=0&purchase_type=all`,
  );
  const summary = data.query_summary ?? {};
  const total = summary.total_reviews ?? 0;
  return {
    review_score: summary.review_score ?? undefined,
    review_desc: summary.review_score_desc ?? undefined,
    reviews_total: total ? total.toLocaleString("en-US") : undefined,
    reviews_percent: summary.total_positive && total
      ? Math.round((summary.total_positive / total) * 100)
      : undefined,
  };
}

async function appDetails(appid, language) {
  const data = await getJson(
    `https://store.steampowered.com/api/appdetails?appids=${appid}&cc=cn&l=${language}`,
  );
  const entry = data?.[String(appid)];
  if (!entry?.success || !entry.data) return null;
  return entry;
}

async function main() {
  console.log(`→ 目标目录 ${OUT_DIR}`);
  await mkdir(OUT_DIR, { recursive: true });

  // 不联网：只用现有 wishlist.json 重新生成 library.json
  if (LIBRARY_ONLY) {
    const raw = await readFile(path.join(OUT_DIR, "wishlist.json"), "utf8");
    const parsed = JSON.parse(raw);
    await writeLibraryFile(parsed.wishlist ?? parsed);
    return;
  }

  const wishlist = {};
  const appdetails = {};
  const now = Math.floor(Date.now() / 1000);
  let index = 0;
  let failures = 0;

  for (const appid of APPIDS) {
    const key = String(appid);
    try {
      const [reviews, english, chinese] = [
        await appReviews(appid),
        await appDetails(appid, "en"),
        await appDetails(appid, "schinese"),
      ];
      if (!english?.data) {
        console.warn(`  skip ${appid}: 没有 appdetails`);
        failures += 1;
        continue;
      }

      appdetails[key] = english;
      if (chinese?.data) appdetails[key] = chinese;

      const data = english.data;
      wishlist[key] = {
        name: data.name,
        capsule: null,
        review_score: reviews.review_score,
        review_desc: reviews.review_desc,
        reviews_total: reviews.reviews_total,
        reviews_percent: reviews.reviews_percent,
        release_string: data.release_date?.date ?? undefined,
        type: data.type ?? "game",
        priority: index,
        // 演示用的「加入愿望单时间」：从现在往前每 6 天一条
        added: now - (index + 1) * 6 * 24 * 3600,
        is_free_game: data.is_free === true,
        tags: (data.genres ?? []).map((genre) => genre.description).filter(Boolean),
        store_url: `https://store.steampowered.com/app/${appid}/`,
      };
      index += 1;
      console.log(
        `  ✓ ${appid} ${data.name} (${data.type}) score=${reviews.review_score ?? "-"}`,
      );
    } catch (error) {
      failures += 1;
      console.warn(`  ✗ ${appid}: ${error.message}`);
    }
    await sleep(350);
  }

  if (index < 5) {
    throw new Error(`成功条目太少（${index}），不覆盖已有 fixture`);
  }

  const wishlistFile = {
    steam_id: "76561198000000000",
    fetched_at: new Date().toISOString(),
    note: "样例数据：由 scripts/gen-steam-fixture.mjs 从公开接口生成，用于离线演示",
    wishlist,
  };

  await writeFile(
    path.join(OUT_DIR, "wishlist.json"),
    `${JSON.stringify(wishlistFile, null, 2)}\n`,
    "utf8",
  );
  await writeFile(
    path.join(OUT_DIR, "appdetails.json"),
    `${JSON.stringify(appdetails, null, 2)}\n`,
    "utf8",
  );
  await writeLibraryFile(wishlist);

  console.log(`✔ 完成：${index} 条愿望单 / ${Object.keys(appdetails).length} 条详情，失败 ${failures}`);
}

main().catch((error) => {
  console.error(`✗ 生成失败: ${error.message}`);
  process.exit(1);
});
