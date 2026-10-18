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
| GET | `/api/games` | List games: `status` `source` `media_kind` `q` `sort` `favorite` `page` `per_page` |
| GET | `/api/games/{id}` | One entry: `{ "game": …, "entry": … }` |
| PATCH | `/api/games/{id}` | Update personal state (status, rating, progress, playtime, note, favorite, priority) |
| DELETE | `/api/games/{id}` | Delete one entry (local-only; a re-import re-creates source items) |
| POST | `/api/games/delete` | Batch delete `{"ids": […]}` → `{"deleted": n}` (multi-select cleanup) |
| POST | `/api/games/{id}/refresh-metadata` | Re-fetch store details for a single entry |
| POST | `/api/imports/steam/wishlist` | Import a Steam wishlist |
| POST | `/api/imports/steam/library` | Import the Steam library (owned games + playtime; requires `STEAM_API_KEY`) |
| POST | `/api/imports` | Generic JSON import (flat `SourceItem`s, merge semantics) |
| POST | `/api/imports/restore` | Restore a backup (the export document verbatim, file wins) |
| GET | `/api/imports` | Run history (`?limit=`) |
| GET | `/api/imports/{id}` | One run, with its warnings |
| POST | `/api/enrich` | Enrich pending entries with store details |
| GET | `/api/export/json` | Full export (games + entries) |

### GET /api/games

`sort` accepts: `added_desc` (default), `added_asc`, `updated_desc`, `title`, `review_desc`,
`rating_desc`, `priority`, `price_desc`, `playtime_desc`. `per_page` is clamped to 1..200.
The response is a page object: `{ items, total, page, per_page, total_pages }`.

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
  "steam_id": "76561197960287930",
  "preview": true,
  "options": { "include_dlc": true, "include_non_games": false, "dry_run": false }
}
```

- `steam_id` accepts a 17-digit steamID64, a vanity name, or a profile URL.
- No API key and no cookie are needed. The importer tries the public wishlist JSON first
  (richer data) and falls back to the official `IWishlistService` endpoint when Steam refuses
  anonymous access; `summary.source_mode` reports which path was used (`public_json`,
  `official_api` or `fixture`).
- `dry_run: true` reports what *would* happen (same numbers as a real run) without writing
  anything; the response then has `"run": null`.
- `include_non_games` adds bundles / software / demos; those are skipped by default.
- Response: `{ "summary": …, "run": …, "preview": [...] }`. `summary.warnings` explains
  fallbacks (for example "fell back to the official API") and `summary.missing` counts entries
  that are no longer in the wishlist — they are never deleted locally.

### POST /api/imports/steam/library

Imports the owned-games library, including cumulative playtime. Same request/response shape as
the wishlist import:

```json
{ "steam_id": "76561197960287930", "preview": true, "options": { "dry_run": false } }
```

- **A Steam Web API key is required** (`STEAM_API_KEY`): the official `GetOwnedGames` endpoint
  refuses anonymous calls (HTTP 401). The community XML game list also redirects anonymous
  visitors to the login page, so there is no keyless fallback. Without a key the request fails
  fast with `400 invalid` and a hint; the web UI disables the library buttons when
  `GET /api/config` reports `steam.has_api_key: false` (fixture mode excepted).
- The profile's **"Game details" privacy setting must be public**, otherwise Steam answers
  `{"response": {}}`. That is reported as `422 source_rejected` with a privacy hint — it is not
  confused with an empty library (which reports 0 items with a warning).
- Owned games are imported with `status = backlog`; a wishlist entry that got bought moves to
  `backlog` through the normal status-adoption rule (a local `playing`/`completed` is never
  reverted). `summary.missing` counts items that were seen in an earlier library sync but are
  gone now (refund / removed / lost family sharing) — with the library wording, not the
  wishlist one.
- **Playtime.** `SourceItem.playtime_minutes` is recorded in
  `entry.source_playtime_minutes`, and `entry.playtime_minutes` follows it while the user has
  not edited it by hand. A manual value wins on every later sync; `PATCH playtime_minutes: -1`
  (or the "adopt Steam playtime" button) restores following.

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

Picks entries that never had store details fetched (`metadata_updated_at IS NULL`), wishlist
first, and fetches them with concurrency 2 and a 400 ms gap. If Steam rate-limits (HTTP 429)
the request fails with 422 and a "try again later" hint; already-fetched items stay committed.
Successful runs are recorded in the run history as `metadata_enrich`.

### POST /api/games/{id}/refresh-metadata

Returns `outcome` = `updated` / `unchanged` / `added` plus the refreshed `game` and `entry`.
Unchanged results do not modify any timestamp. A rate limit is reported as 422, a missing store
page as 502.

### GET /api/export/json

```json
{ "exported_at": "…", "app": "acgn-tools", "schema_revision": 2, "count": 27, "items": [ … ] }
```

Each element is `{ "game": …, "entry": … }`: full metadata plus personal state. The document is
meant to be posted back verbatim to [`POST /api/imports/restore`](#post-apiimportsrestore) for a
backup round-trip (metadata + personal state; bookkeeping timestamps are refreshed). Flat
`SourceItem` payloads for the merge import go to `POST /api/imports` instead.
