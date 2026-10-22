# HTTP API

Base path: `/api`. Every payload is JSON with `snake_case` keys, mirroring the Rust DTOs in
`crates/acgn-core/src/dto.rs`. `GET /api` returns a self-describing index of all endpoints.

Runnable samples for every endpoint live in [`api.http`](api.http) (VS Code REST Client).

## Conventions

**Errors.** Every failure — including bad input — uses the same envelope:

```json
{ "error": { "kind": "invalid", "message": "…", "hint": "请检查输入" } }
```

| HTTP | `kind` | When |
|---|---|---|
| 400 | `invalid` | malformed query / body / path (bad enum value, non-numeric id, …) |
| 404 | `not_found` | unknown resource or route |
| 422 | `source_rejected` | upstream refused (private profile, rate limit) — retry later |
| 502 | `network` / `upstream` | upstream unreachable or returned something unparsable |
| 500 | `storage` / `other` | database or unexpected failure |

`hint` is a short, user-facing suggestion; the UI shows it under the error message.

**Status keys.** `wishlist`, `backlog`, `playing`, `completed`, `on_hold`, `dropped`, `ignored`.

**PATCH semantics** (used by the game endpoints): a missing field means "leave unchanged";
a negative number clears a numeric field; an empty `note` clears it. Moving the status to
`playing` / `completed` fills `started_at` / `finished_at` (and sets progress to 100 for
`completed`) when those are still empty. Clearing `playtime_minutes` (negative) drops the local
manual value: when a tracked Steam playtime exists the effective value reverts to it (see the
library import section).

**Imports are idempotent.** Re-running the same import reports `unchanged` and writes nothing.
Metadata follows the source; personal state follows the local database — see
[`design.md`](design.md) §4.

## Endpoints

| Method | Path | Description |
|---|---|---|
| GET | `/api` | Endpoint index |
| GET | `/api/health` | Liveness probe, version, uptime, data-source mode |
| GET | `/api/config` | Frontend config: status list, Steam mode, country/language |
| GET | `/api/stats` | Dashboard aggregates |
| GET | `/api/news` | Game news across owned Steam games, newest first (`page`, `per_page`, `game_id`) |
| GET | `/api/news/status` | News-source coverage: owned / synced / pending / failed / item counts |
| GET | `/api/news/sync/status` | Live progress of the current / last full news sync (`done`, `total`, ETA, …) |
| POST | `/api/news/sync` | Start a background full sync of every owned game's news feed |
| GET | `/api/accounts` | Source accounts with owned / wishlist counts |
| POST | `/api/accounts/resolve` | Add an account: resolve steamID / vanity / profile link, fetch its profile (idempotent) |
| PATCH | `/api/accounts/{id}` | Set or clear the account's regional price override |
| DELETE | `/api/accounts/{id}` | Delete an account (associations cascade; games stay) |
| GET | `/api/games` | List games: `status` `source` `media_kind` `account_id` `q` `sort` `favorite` `page` `per_page` |
| GET | `/api/games/{id}` | One entry: `{ "game": …, "entry": …, "accounts": […] }` |
| PATCH | `/api/games/{id}` | Update personal state (status, rating, progress, playtime, note, favorite, priority) |
| DELETE | `/api/games/{id}` | Delete one entry (local-only; a re-import re-creates source items) |
| POST | `/api/games/delete` | Batch delete `{"ids": […]}` → `{"deleted": n}` (multi-select cleanup) |
| POST | `/api/games/{id}/refresh-metadata` | Re-fetch store details for a single entry |
| POST | `/api/imports/steam/wishlist` | Import a Steam wishlist for an account |
| POST | `/api/imports/steam/library` | Import the Steam library (owned games + playtime; requires `STEAM_API_KEY`) |
| POST | `/api/imports/steam/achievements` | Check achievement progress (all achievements → completed; per-game lookups) |
| POST | `/api/imports` | Generic JSON import (flat `SourceItem`s, merge semantics) |
| POST | `/api/imports/restore` | Restore a backup (the export document verbatim, file wins) |
| GET | `/api/imports` | Run history (`?limit=`) |
| GET | `/api/imports/{id}` | One run, with its warnings |
| POST | `/api/enrich` | Enrich pending entries with store details + account-region prices |
| GET | `/api/export/json` | Full export (games + entries) |

### Accounts

Source accounts are **Steam identities**, not application logins. `POST /api/accounts/resolve`
creates one row per `(source, external_id)` and refreshes the profile (best-effort):

```json
{ "source": "steam", "input": "76561197960287930" }
```

`input` accepts a 17-digit steamID64, a vanity name or a profile URL. The response carries the
stored profile (persona name, avatar, profile URL, region, raw payload), `created`, `resolved_via`
and any warnings:

```json
{
  "account": {
    "id": 1, "source": "steam", "external_id": "76561197960287930",
    "display_name": "…", "avatar_url": "…", "profile_url": "…",
    "country_code": "cn", "country_override": null, "effective_country": "cn",
    "metadata": { "…": "raw GetPlayerSummaries player object" },
    "metadata_updated_at": "…", "last_synced_at": "…", "created_at": "…", "updated_at": "…"
  },
  "created": true,
  "resolved_via": "numeric",
  "warnings": []
}
```

`GET /api/accounts` returns every account plus `owned_count` / `wishlist_count`. `PATCH` only
takes `country_code` (two letters, e.g. `us`; empty string clears the override and follows the
profile); changing the effective region clears that account's cached prices. `DELETE` returns
`204`, removes only `game_accounts` rows, and keeps the games / personal state.

### Game news

News is collected from Steam's per-appid announcement RSS and archived locally. Only **owned**
games are polled: a game counts as owned when an account link has
`source_playtime_minutes` (0 included), or for legacy rows without account links, when the
library entry has `source_playtime_minutes`. Wishlist-only entries are ignored.

#### GET /api/news

`page` (default 1), `per_page` (default 20, max 100) and optional `game_id`:

```json
{
  "items": [
    {
      "id": 7,
      "game_id": 12,
      "source": "steam",
      "external_id": "1245620",
      "game_title": "ELDEN RING",
      "game_title_zh": "艾尔登法环",
      "game_cover_url": "…",
      "game_poster_url": "…",
      "language": "schinese",
      "title": "Patch notes",
      "summary": "Full plain-text body (HTML stripped, not truncated)",
      "url": "https://steamcommunity.com/games/1245620/announcements/detail/…",
      "author": "steam",
      "published_at": "2026-10-20T12:00:00Z",
      "fetched_at": "2026-10-22T07:00:00Z"
    }
  ],
  "total": 1, "page": 1, "per_page": 20, "total_pages": 1
}
```

Results are ordered by `published_at DESC` (ties by id). The page reads SQLite only; it never
performs RSS requests.

#### GET /api/news/status

```json
{
  "language": "schinese",
  "owned_games": 42,
  "synced_games": 30,
  "pending_games": 12,
  "failed_games": 1,
  "news_items": 315,
  "last_synced_at": "2026-10-22T07:00:00Z"
}
```

`pending_games` = owned − ever-succeeded. `failed_games` counts feeds whose most recent attempt
failed; they keep their previously archived news and are retried on the next sync.

#### POST /api/news/sync

Starts one **background pass over every owned game** and returns immediately. There is no batch
size and no request body. Calling it while a pass is already running returns the current progress
instead of starting another one.

```json
{
  "running": true,
  "total": 42,
  "done": 0,
  "succeeded": 0,
  "failed": 0,
  "items_added": 0,
  "items_updated": 0,
  "items_unchanged": 0,
  "current_game": null,
  "started_at": "2026-10-22T07:00:00Z",
  "finished_at": null,
  "elapsed_ms": 0,
  "eta_seconds": null,
  "warnings": []
}
```

Each feed is fetched sequentially and written as soon as it finishes, so `GET /api/news` already
shows new items while the pass is running. `done` counts finished feeds (success **and** failure);
`eta_seconds` is estimated from the average time per finished feed.

- A missing / delisted feed (HTTP 404), or an HTML page for an app without a community hub
  (Portal, Portal 2, …), is treated as an empty feed. Other per-game failures are reported in
  `warnings` / `failed` without failing the whole pass. Three consecutive failures stop the pass
  early; the remaining feeds are picked up on the next run.
- The full announcement text is stored (HTML stripped, paragraph breaks preserved) — the API does
  not truncate `summary`. The web UI shows an excerpt in the list and the full text in a modal.
- A `steam_news` run is written to `import_runs` (`seen` = feed attempts, `added` / `updated` /
  `unchanged` = news rows, `skipped` = failed feeds). If every attempted feed fails the run is
  marked `failed`. With no owned games, `running` is `false`, `total` is `0` and a warning is
  returned; no run row is written.
- RSS request language / region follow `STEAM_LANGUAGE` / `STEAM_COUNTRY` (default
  `schinese` / `cn`). The language is part of the archive key, so changing it hides the previous
  language's rows instead of deleting them.

#### GET /api/news/sync/status

Returns the same shape as the POST response, for polling. The state is kept in memory: after a
server restart it resets to `running: false`, while already-fetched news and coverage remain in
SQLite. The UI polls every ~600 ms, updates the progress bar / ETA, and silently refreshes the
current list page whenever `done` advances.

### Steam imports are per account

Each Steam import either selects an existing account or supplies a raw ID (which creates the
account automatically). Prefer `account_id`:

```json
{ "account_id": 1, "preview": true, "options": { "dry_run": false } }
```

`steam_id` remains supported for scripts and one-off syncs:

```json
{ "steam_id": "76561197960287930", "preview": true }
```

Either way the run args record `account_id` / `account`, and each imported item is linked to the
account (`in_wishlist` for wishlist imports, per-account playtime for library imports). Omitting
both → `400 invalid`.

### GET /api/games

`sort` accepts: `added_desc` (default), `added_asc`, `updated_desc`, `title`, `review_desc`,
`rating_desc`, `priority`, `price_desc`, `playtime_desc`. `per_page` is clamped to 1..200.
The response is a page object: `{ items, total, page, per_page, total_pages }`.

`account_id` restricts the list to games associated with that account (its wishlist **and** its
library). Every item carries an optional `accounts` array (omitted when empty) with the linked
accounts, per-account playtime / last played and the account-region price:

```json
{
  "game": { "id": 12, "title": "…" },
  "entry": { "playtime_minutes": 120 },
  "accounts": [
    {
      "account_id": 1, "external_id": "7656119…", "display_name": "…", "avatar_url": "…",
      "country": "cn", "owned": true, "in_wishlist": false,
      "source_playtime_minutes": 120, "playtime_2weeks_minutes": 0, "last_played_at": "…",
      "price_currency": null, "price_initial": null, "price_final": null,
      "price_updated_at": null, "last_synced_at": "…"
    }
  ]
}
```

`q` matches title, Chinese title, developers, publishers and tags. Wildcards in user input
(`%`, `_`) are treated as literals.

### DELETE /api/games/{id}

Deletes one entry. This is a **local action**, deliberately separate from sync
([`design.md`](design.md) §4):

- the `games` row is removed and `library_entries` cascades, so personal state (rating, note,
  playtime, favorite, …) is lost with it;
- no `import_runs` row is written — a deletion is not an import;
- if the source still lists the item, the next import re-creates it as `added` with a new local
  id. "Deleted" is not "ignored": use the `ignored` status to keep it out of your way instead;
- unknown id → `404`; success → `204 No Content`.

### POST /api/games/delete

Batch variant used by the library's multi-select mode:

```json
{ "ids": [12, 13, 14] }
```

The batch runs in one transaction, unknown ids are ignored and the response reports what
actually happened — `{ "deleted": 2 }`. Empty `ids` or more than 1000 ids → `400`.

### POST /api/imports/steam/wishlist

```json
{
  "account_id": 1,
  "preview": true,
  "options": { "include_dlc": true, "include_non_games": false, "dry_run": false }
}
```

- `account_id` selects a stored account (recommended). `steam_id` (17-digit steamID64, vanity
  name or profile URL) is still accepted and creates the account automatically; the run args
  record `account_id` / `account`.
- No API key and no cookie are needed. The importer tries the public wishlist JSON first
  (richer data) and falls back to the official `IWishlistService` endpoint when Steam refuses
  anonymous access; `summary.source_mode` reports which path was used (`public_json`,
  `official_api` or `fixture`).
- On the official path the response only carries appids, so the importer batch-fills localized
  names, item types and vertical posters from `IStoreBrowseService/GetItems` (50 per request)
  before saving. Only items the catalog cannot resolve (delisted / region-hidden) keep the
  `Steam App <id>` placeholder; prices and descriptions still need "enrich".
- `dry_run: true` reports what *would* happen (same numbers as a real run) without writing
  anything; the response then has `"run": null`.
- `include_non_games` adds bundles / software / demos; those are skipped by default.
- Every accepted item is linked to the account (`in_wishlist`); a library sync for the same
  account clears that flag. `summary.missing` counts items that are gone from **this account's**
  wishlist — other accounts and the local rows are untouched.

### POST /api/imports/steam/library

Imports the owned-games library, including cumulative playtime. Same request/response shape as
the wishlist import:

```json
{ "account_id": 1, "preview": true, "options": { "dry_run": false } }
```

- **A Steam Web API key is required** (`STEAM_API_KEY`): the official `GetOwnedGames` endpoint
  refuses anonymous calls (HTTP 401). The community XML game list also redirects anonymous
  visitors to the login page, so there is no keyless fallback. Without a key the request fails
  fast with `400 invalid` and a hint; the web UI disables the library buttons when
  `GET /api/config` reports `steam.has_api_key: false` (fixture mode excepted).
- The profile's **"Game details" privacy setting must be public**, otherwise Steam answers
  `{"response": {}}`. That is reported as `422 source_rejected` with a privacy hint — it is not
  confused with an empty library (which reports 0 items with a warning).
- Owned games with playtime are imported as `playing`, never-played ones as `backlog`. A
  `wishlist` / `backlog` entry is upgraded through the normal status-adoption rule; a status
  the user set (`playing` / `completed` / …) is never reverted.
- Each item is linked to the account with its own `source_playtime_minutes`,
  `playtime_2weeks_minutes` and `last_played_at`. The entry-level tracked playtime is the
  **maximum across accounts**, so a low-playtime alt cannot erase the main account's hours.
  `summary.missing` is scoped to this account (refund / removed / lost family sharing).
- **Playtime.** `entry.source_playtime_minutes` is the aggregate; `entry.playtime_minutes`
  follows it while the user has not edited it by hand. A manual value wins on every later sync;
  `PATCH playtime_minutes: -1` (or the "adopt Steam playtime" button) restores following.
- If every imported game shows playtime `0`, the account has *"Always keep my total playtime
  private"* enabled (a checkbox under the Game-details privacy setting) — ownership is still
  imported. `playtime_2weeks` / `rtime_last_played` may also be absent unless the profile is
  the API key's owner; they are optional fields.

### POST /api/imports/steam/achievements

Checks achievement progress, one game per request, and marks games with **all achievements**
as `completed`:

```json
{ "account_id": 1, "limit": 200 }
```

- `account_id` selects the account (recommended); `steam_id` is accepted as a compatibility
  entry and auto-creates the account. Omitting both → `400`.
- Requires `STEAM_API_KEY` and a public "Game details" setting (same as the library import).
  `limit` defaults to 200 (max 500) — the response's `pending` tells you how many entries were
  never checked; call again to continue.
- Only entries owned by the selected account (`game_accounts.source_playtime_minutes`
  non-null) are checked. Poll order is never-checked first, then the least-recently checked, so
  refreshes rotate.
- Status rules: `wishlist` / `backlog` / `playing` are promoted to `completed` (filling
  `finished_at` and 100% progress when empty); a user-set `completed` / `on_hold` / `dropped` /
  `ignored` is never reverted. Games without an achievement set stay untouched.
- Per-entry progress is stored in `entry.achievements_achieved` / `achievements_total` and
  `achievements_checked_at`; entries whose achievements cannot be read (not owned / private)
  keep their previous values and still advance the polling clock. Progress is entry-level: when
  several accounts own the same game, the displayed value is from the most recently checked
  account, while any account's full completion promotes the entry.
- Response:

```json
{ "checked": 24, "updated": 18, "promoted": 6, "unchanged": 6, "unavailable": 80, "failed": 0,
  "pending": 21, "source_mode": "live_with_key", "duration_ms": 9123,
  "warnings": ["6 个游戏全成就，已标记为「已通关」"] }
```

`checked` counts entries with real achievement data (this account owns them); `updated` counts
entries whose stored values actually changed (promotions are a subset); `unavailable` means the
account does not own that game (still checked, progress kept); `failed` is a real request error
and is retried on the next click. Five consecutive failures stop the run (systemic failure
protection) and are reported in `warnings`.

- Recorded in the run history as `steam_achievements` (also when the run stops early / fails).
  In this mode `skipped` counts request failures only — entries the account does not own are
  normal answers and appear in the warnings, not in `skipped`.

### POST /api/imports

Accepts flat `SourceItem`s — the shape a new source adapter produces:

```json
{
  "source": "steam",
  "preview": true,
  "options": { "dry_run": false },
  "items": [ { "external_id": "1245620", "title": "ELDEN RING", "status": "wishlist" } ]
}
```

Two similarly named switches, **not** the same thing:

| Field | Meaning |
|---|---|
| `preview` | return the affected entries in the response `preview` array (default `true`) |
| `options.dry_run` | **do not write anything**: no rows, no run record, `run: null` |

A request-level `source` overrides each item's own `source`; mixing several sources without it is
rejected with 400.

### POST /api/imports/restore

Restores a backup produced by `GET /api/export/json`. The **whole export document can be posted
verbatim** (extra fields such as `exported_at` or `count` are ignored); a bare array of
`{ "game": …, "entry": … }` items is accepted too.

```json
{ "items": [ { "game": { "source": "steam", "external_id": "1245620", … }, "entry": { "status": "playing", "rating": 9, … } } ], "dry_run": false }
```

Restore semantics differ from a sync on purpose (**file wins**):

- metadata *and* personal state (status, rating, note, progress, playtime, favorite, priority)
  come from the file — this is a restore, not a merge;
- entries missing from the file are left untouched (nothing is deleted);
- `created_at` / `updated_at` / `last_synced_at` are bookkeeping and are refreshed to "now", so a
  restore is reversible at the level of "metadata + personal state", not byte-for-byte;
- `metadata_json` (the cached raw upstream payload) is not part of the export and keeps its value;
- `dry_run: true` reports the same added / updated / unchanged counts without writing anything.

Re-running the same file is idempotent: the second run reports `unchanged` for every entry.

### POST /api/enrich

```json
{ "limit": 20 }
```

Two passes, both throttled by the same configuration (`STEAM_ENRICH_CONCURRENCY` /
`STEAM_ENRICH_DELAY_MS`) and both stopping gracefully on HTTP 429:

1. **Metadata** — entries that never had store details fetched (`metadata_updated_at IS NULL`),
   wishlist first, fetched with the configured `STEAM_COUNTRY`.
2. **Account-region prices** — `game_accounts` rows whose account region differs from
   `STEAM_COUNTRY` and whose `price_updated_at IS NULL`. Requests are grouped by country and
   appid (two accounts in one region share a fetch) and written to that account's link. Accounts
   in the configured region (or without a known region) are not fetched separately; the UI falls
   back to the global price for them.

The response reports `account_prices` (association rows resolved, including "no listing in this
region") next to the metadata counts. Batches that answer normally but lack an appid mark that
association as checked so the pass converges; request-level failures stay pending for the next
call. Run history counts both passes: `seen`/`updated`/`missing` include the region-price
targets, so a run that only wrote prices no longer shows `updated: 0`. Successful runs are recorded in the run history as
`metadata_enrich`. A rate limit during the metadata pass fails with 422 and leaves already-fetched
items committed; a rate limit during the price pass only stops that pass and is reported in
`warnings`.

### POST /api/games/{id}/refresh-metadata

Returns `outcome` = `updated` / `unchanged` / `added` plus the refreshed `game`, `entry` and
`accounts`. Unchanged results do not modify any timestamp. A rate limit is reported as 422, a
missing store page as 502.

### GET /api/export/json

```json
{ "exported_at": "…", "app": "acgn-tools", "schema_revision": 3, "count": 27, "items": [ … ] }
```

Each element is `{ "game": …, "entry": … }`: full metadata plus personal state. The document is
meant to be posted back verbatim to [`POST /api/imports/restore`](#post-apiimportsrestore) for a
backup round-trip (metadata + personal state; bookkeeping timestamps are refreshed). Flat
`SourceItem` payloads for the merge import go to `POST /api/imports` instead.

Accounts and their game associations are **not** part of the export: they are derived from the
source and are rebuilt by the next account sync (avatars / names come back with the profile
refresh).
