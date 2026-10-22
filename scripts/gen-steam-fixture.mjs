#!/usr/bin/env node
/**
 * 生成 Steam 离线 fixture（给开发 / 演示 / 回归测试用）。
 *
 * 数据来源（公开接口，不需要 API Key）：
 *   - appreviews  → 好评档位 / 好评数（真实）
 *   - appdetails  → 中英文名 / 价格 / 简介 / 发售日 / 平台（真实）
 *   - library.json / achievements.json 的时长与成就是脚本**确定性生成**的演示数据
 *     （GetOwnedGames / GetPlayerAchievements 需要 API Key 与真实账号，
 *     仓库里不能放任何人的真实库存），名字来自 appdetails。
 *
 * 用法：
 *   node scripts/gen-steam-fixture.mjs                 # 生成到 fixtures/steam/
 *   node scripts/gen-steam-fixture.mjs --library-only  # 只用现有 wishlist.json 生成 library/achievements（不联网）
 *   node scripts/gen-steam-fixture.mjs --news-only     # 只抓新闻 RSS（联网；appid 来自 library/wishlist）
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
/** 只写 profile.json：不联网，给账号演示补资料 */
const PROFILE_ONLY = args.includes("--profile-only");
/** 只抓新闻 RSS：联网；appid 来自现有 library.json（缺失时退回 wishlist.json） */
const NEWS_ONLY = args.includes("--news-only");

const NEWS_COUNTRY = "cn";
const NEWS_LANGUAGE = "schinese";
/** fixture 每个游戏保留最近几条新闻，避免样例文件过大 */
const NEWS_PER_GAME = 5;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * 演示用账号资料（profile.json）。
 * 头像用内联 SVG data URI：离线 demo 不联网也能看到头像，浏览器也走同一条 <img> 路径。
 */
function buildProfileFile() {
  const now = Math.floor(Date.now() / 1000);
  const avatar = `data:image/svg+xml,${encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96">' +
      '<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">' +
      '<stop offset="0" stop-color="#1b2838"/><stop offset="1" stop-color="#2a475e"/>' +
      "</linearGradient></defs>" +
      '<rect width="96" height="96" rx="10" fill="url(#g)"/>' +
      '<text x="48" y="62" font-family="system-ui,sans-serif" font-size="40" ' +
      'text-anchor="middle" fill="#66c0f4">A</text></svg>',
  )}`;

  return {
    steam_id: "76561198000000000",
    // appdetails.json 是按这个区域（cc=cn）抓的：其它区域不能拿它冒充区域价格
    country: "cn",
    fetched_at: new Date().toISOString(),
    note: "样例数据：账号资料由 scripts/gen-steam-fixture.mjs 生成（演示用，不是真实账号）",
    player: {
      steamid: "76561198000000000",
      personaname: "ACGN Demo",
      profileurl: "https://steamcommunity.com/profiles/76561198000000000/",
      avatar,
      avatarmedium: avatar,
      avatarfull: avatar,
      avatarhash: "demo",
      personastate: 0,
      communityvisibilitystate: 3,
      profilestate: 1,
      timecreated: now - 10 * 365 * 24 * 3600,
      lastlogoff: now - 24 * 3600,
      loccountrycode: "CN",
    },
  };
}

async function writeProfileFile() {
  const profile = buildProfileFile();
  await writeFile(
    path.join(OUT_DIR, "profile.json"),
    `${JSON.stringify(profile, null, 2)}\n`,
    "utf8",
  );
  console.log(`✔ profile.json：${profile.player.personaname}（演示头像）`);
}

/** 演示用累计时长（分钟）：确定性生成 —— 前几条像在玩，其余多为未玩。 */
function demoPlaytime(index) {
  return index % 5 === 0 ? 0 : (index * 173) % 6000;
}

/**
 * 从 wishlist 生成 GetOwnedGames 形态的 library.json。
 * 时长是演示数据；条目标记「已拥有」，导入后会按「有时长 = 在玩」处理。
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

/**
 * 从 library 生成 achievements.json（GetPlayerAchievements 的汇总形态）。
 * 覆盖**每一个**库存条目，与真实接口语义一致：
 *   - 玩过的：每 3 个里 1 个全成就，其余部分进度；
 *   - 没玩过的：0/0（真实接口回「没有统计」→ 0/0，而不是「取不到」）。
 */
function buildAchievementsFile(libraryFile) {
  const achievements = {};
  let played = 0;
  for (const game of libraryFile.response.games) {
    if (!game.playtime_forever) {
      achievements[String(game.appid)] = { achieved: 0, total: 0 };
      continue;
    }
    const total = 12 + ((played * 7) % 30);
    const achieved =
      played % 3 === 0 ? total : Math.floor((total * ((played % 5) + 1)) / 6);
    achievements[String(game.appid)] = { achieved, total };
    played += 1;
  }
  return {
    note: "样例数据：成就进度由 scripts/gen-steam-fixture.mjs 确定性生成（演示用，不是真实账号数据）",
    achievements,
  };
}

async function writeLibraryFile(wishlist) {
  const libraryFile = buildLibraryFile(wishlist);
  await writeFile(
    path.join(OUT_DIR, "library.json"),
    `${JSON.stringify(libraryFile, null, 2)}\n`,
    "utf8",
  );
  const achievementsFile = buildAchievementsFile(libraryFile);
  await writeFile(
    path.join(OUT_DIR, "achievements.json"),
    `${JSON.stringify(achievementsFile, null, 2)}\n`,
    "utf8",
  );
  console.log(
    `✔ library.json：${libraryFile.response.game_count} 条（演示时长）；` +
      `achievements.json：${Object.keys(achievementsFile.achievements).length} 条（演示成就）`,
  );
  await writeProfileFile();
}

const HTML_ENTITIES = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  nbsp: " ",
  trade: "™",
  copy: "©",
  reg: "®",
  deg: "°",
  plusmn: "±",
  times: "×",
  divide: "÷",
  frac12: "½",
  frac14: "¼",
  frac34: "¾",
  sup2: "²",
  sup3: "³",
  micro: "µ",
  para: "¶",
  sect: "§",
  middot: "·",
  bull: "•",
  hellip: "…",
  ndash: "–",
  mdash: "—",
  lsquo: "‘",
  rsquo: "’",
  ldquo: "“",
  rdquo: "”",
  laquo: "«",
  raquo: "»",
  euro: "€",
  pound: "£",
  yen: "¥",
  cent: "¢",
  larr: "←",
  rarr: "→",
  uarr: "↑",
  darr: "↓",
  hearts: "♥",
  spades: "♠",
  clubs: "♣",
  diams: "♦",
};

/** XML / HTML 实体 → 字符（只解一层，和 Rust 侧解析保持同一口径）。 */
function decodeXmlEntities(value) {
  return value.replace(/&(#x?[0-9a-fA-F]+|[a-zA-Z][a-zA-Z0-9]+);/g, (raw, entity) => {
    const hex = entity.match(/^#x([0-9a-fA-F]+)$/);
    if (hex) return String.fromCodePoint(Number.parseInt(hex[1], 16));
    if (entity.startsWith("#")) return String.fromCodePoint(Number.parseInt(entity.slice(1), 10));
    return HTML_ENTITIES[entity] ?? raw;
  });
}

/** 取一个简单标签的文本，支持 CDATA 与属性（如 `<guid isPermaLink="true">`）。 */
function readTag(block, name) {
  const match = block.match(
    new RegExp(`<${name}(?:\\s[^>]*)?>([\\s\\S]*?)<\\/${name}>`, "i"),
  );
  if (!match) return "";
  return decodeXmlEntities(match[1].replace(/<!\[CDATA\[([\s\S]*?)\]\]>/g, "$1")).trim();
}

/** HTML → 纯文本（保留段落 / 换行，不截断；与 Rust `html_to_text` 口径一致）。 */
function htmlToText(raw) {
  if (!raw) return undefined;
  const stripped = raw
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/<\/(p|div|li|tr|h[1-3])>/gi, "\n\n")
    .replace(/<[^>]*>/g, "");
  const decoded = decodeXmlEntities(stripped).replace(/\r\n?/g, "\n");
  const text = decoded
    .split("\n")
    .map((line) => line.replace(/[ \t]+/g, " ").trim())
    .join("\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
  return text || undefined;
}

/** 解析 Steam 公告 RSS：只保留最近 NEWS_PER_GAME 条，坏条目跳过。 */
function parseNewsFeed(xml) {
  const entries = [];
  for (const match of xml.matchAll(/<item>([\s\S]*?)<\/item>/gi)) {
    const block = match[1];
    const title = readTag(block, "title");
    const link = readTag(block, "link");
    const guid = readTag(block, "guid") || link;
    const pubDate = new Date(readTag(block, "pubDate"));
    if (!title || !link || !guid || Number.isNaN(pubDate.getTime())) continue;

    const author = readTag(block, "author");
    entries.push({
      guid,
      title,
      summary: htmlToText(readTag(block, "description")),
      url: link,
      author: author || undefined,
      published_at: pubDate.toISOString(),
    });
    if (entries.length >= NEWS_PER_GAME) break;
  }
  return entries;
}

async function getText(url, attempt = 1) {
  try {
    const response = await fetch(url, {
      headers: {
        accept: "application/rss+xml, application/xml, text/xml",
        "accept-language": NEWS_LANGUAGE,
        "user-agent": "acgn-tools fixture generator",
      },
    });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return await response.text();
  } catch (error) {
    if (attempt >= 3) throw error;
    await sleep(1000 * attempt);
    return getText(url, attempt + 1);
  }
}

/** 抓取已拥有游戏的新闻 RSS，写成 news.json（每个 appid 最近几条）。 */
async function writeNewsFile(appids) {
  const news = {};
  let ok = 0;
  let failed = 0;

  for (const appid of appids) {
    try {
      const url =
        `https://steamcommunity.com/games/${appid}/rss/` +
        `?cc=${NEWS_COUNTRY}&l=${NEWS_LANGUAGE}`;
      const entries = parseNewsFeed(await getText(url));
      if (entries.length) {
        news[String(appid)] = entries;
        ok += 1;
        console.log(`  ✓ news ${appid}: ${entries.length} 条`);
      } else {
        console.log(`  · news ${appid}: 没有公告`);
      }
    } catch (error) {
      failed += 1;
      console.warn(`  ✗ news ${appid}: ${error.message}`);
    }
    await sleep(250);
  }

  if (ok === 0) {
    throw new Error(`没有抓到任何新闻（失败 ${failed}），不覆盖已有 news.json`);
  }

  const file = {
    note: "样例数据：游戏公告来自 Steam Community RSS，由 scripts/gen-steam-fixture.mjs 生成（演示用）",
    country: NEWS_COUNTRY,
    language: NEWS_LANGUAGE,
    fetched_at: new Date().toISOString(),
    news,
  };
  await writeFile(
    path.join(OUT_DIR, "news.json"),
    `${JSON.stringify(file, null, 2)}\n`,
    "utf8",
  );
  console.log(`✔ news.json：${ok} 个游戏有新闻，失败 ${failed}`);
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

/** 现有 fixture 里的 appid 列表（library 优先，缺失时用 wishlist）。 */
async function readExistingAppids() {
  try {
    const library = JSON.parse(
      await readFile(path.join(OUT_DIR, "library.json"), "utf8"),
    );
    const games = library.response?.games ?? library.games ?? library.library ?? library.items ?? [];
    const appids = games.map((game) => game.appid).filter(Boolean);
    if (appids.length) return [...new Set(appids.map(String))];
  } catch {
    // library.json 不存在：继续尝试 wishlist.json
  }

  const wishlist = JSON.parse(
    await readFile(path.join(OUT_DIR, "wishlist.json"), "utf8"),
  );
  return Object.keys(wishlist.wishlist ?? wishlist);
}

async function main() {
  console.log(`→ 目标目录 ${OUT_DIR}`);
  await mkdir(OUT_DIR, { recursive: true });

  // 不联网：只补账号资料
  if (PROFILE_ONLY) {
    await writeProfileFile();
    return;
  }

  // 不联网：只用现有 wishlist.json 重新生成 library.json
  if (LIBRARY_ONLY) {
    const raw = await readFile(path.join(OUT_DIR, "wishlist.json"), "utf8");
    const parsed = JSON.parse(raw);
    await writeLibraryFile(parsed.wishlist ?? parsed);
    return;
  }

  // 联网：只刷新新闻。优先用已拥有的 library.json，缺失时退回 wishlist。
  if (NEWS_ONLY) {
    const appids = await readExistingAppids();
    if (appids.length === 0) {
      throw new Error("library.json / wishlist.json 里没有 appid，无法生成新闻 fixture");
    }
    await writeNewsFile(appids);
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
  try {
    // 刚写完 library.json：完整生成也用「库存 appid」抓新闻，和 --news-only 保持一致，
    // 避免只覆盖愿望单、漏掉仅库存拥有的游戏。
    await writeNewsFile(await readExistingAppids());
  } catch (error) {
    // 新闻是附加样例：RSS 全都失败时保留旧 news.json，不让整次 fixture 生成失败
    console.warn(`⚠ news.json 未更新：${error.message}`);
  }

  console.log(`✔ 完成：${index} 条愿望单 / ${Object.keys(appdetails).length} 条详情，失败 ${failures}`);
}

main().catch((error) => {
  console.error(`✗ 生成失败: ${error.message}`);
  process.exit(1);
});
