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
`completed`) when those are still empty.

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
| POST | `/api/games/{id}/refresh-metadata` | Re-fetch store details for a single entry |
| POST | `/api/imports/steam/wishlist` | Import a Steam wishlist |
| POST | `/api/imports` | Generic JSON import (offline / restore) |
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
{ "exported_at": "…", "app": "acgn-tools", "schema_revision": 1, "count": 27, "items": [ … ] }
```

The `items` array is accepted verbatim by `POST /api/imports` (`{ "items": [...] }`), which
makes export → import a lossless backup round-trip. A JSON import may set a request-level
`source` (it then overrides every item); mixing several sources in one request without it is
rejected with 400.
