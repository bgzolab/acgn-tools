# Design notes

How `acgn-tools` is built and which rules the implementation must not break.
User-facing setup lives in the [README](../README.md); the HTTP surface is in
[`api.md`](api.md).

## 1. Goals

- **Own your data**: one SQLite file, no accounts, no third-party service in the write path.
- **Import from community sources without a new pipeline per source** — Steam today, Bangumi
  and Xiaoheihe (小黑盒) later — by normalizing everything into one internal contract.
- **Small enough to self-host**: a single binary that serves the API and the built frontend,
  and that a future Tauri desktop build can embed as a library.

Non-goals for now: multi-user accounts, remote authentication, mobile apps.

## 2. Architecture

```
                        ┌──────────────────────────────────────┐
    browser / Tauri ──▶ │        web (Vue 3 + Vite)            │
                        └──────────────────┬───────────────────┘
                                           │ HTTP /api
                        ┌──────────────────▼───────────────────┐
                        │  acgn-server (axum): REST + static   │
                        └───────┬──────────────────────┬───────┘
                                │                      │
                  ┌─────────────▼──────────┐  ┌────────▼────────────────┐
                  │ acgn-store (SQLite)    │  │ acgn-steam (adapters)   │
                  └─────────────┬──────────┘  └────────┬────────────────┘
                                │                      │
                        ┌───────▼──────────────────────▼───────┐
                        │  acgn-core: domain + SourceItem      │
                        └──────────────────────────────────────┘
```

| Crate | Responsibility | Must not |
|---|---|---|
| `acgn-core` | Domain models (`Game`, `LibraryEntry`), DTOs, error type, `SourceItem` contract | Depend on `sqlx` / `reqwest` / `axum` |
| `acgn-store` | Migrations, queries, import merge, run history | Know anything about Steam-specific JSON |
| `acgn-steam` | Steam adapters (public JSON → official API → fixture) | Write to the database |
| `acgn-server` | REST routes, error mapping, static hosting | Contain business rules (they live in `acgn-store`) |
| `web` | Vue 3 SPA talking to `/api` only | Assume a Tauri/desktop API exists |

Two deliberate choices:

- `acgn-server` also ships a **library target** (`src/lib.rs`). Integration tests build the
  router directly, and a future Tauri client can run the same axum app in-process instead of
  spawning a separate service.
- The frontend only ever talks HTTP. Whether it is served by `acgn-server`, by Vite, or inside
  a Tauri webview, the contract is identical.

## 3. Data model

| Table | Meaning |
|---|---|
| `games` | A catalog item: metadata that belongs to "the world" (title, cover, price, review, …), unique per `(source, external_id)` |
| `library_entries` | My relationship to a game: status, rating, progress, playtime, note, favorite, plus sync bookkeeping. 1:1 with `games` today |
| `import_runs` | One row per import / enrichment run: counts, warnings, duration, error |

Rationale:

- Splitting *catalog* from *my state* keeps the merge rules simple and testable, and leaves
  room for a future `holdings` table (the same game owned on several platforms) without
  touching either core model.
- Money is stored as integer **cents** plus a currency code; floats never touch prices.
- Timestamps are RFC3339 `TEXT` written by `sqlx` (consistent format, lexicographically
  sortable). Array-ish columns (`developers`, `tags`, …) are JSON strings.
- `games.metadata_json` keeps the raw payload of the most authoritative source seen so far
  (see §4.2) — useful for debugging and for backfilling fields later without a re-fetch.
- Enum-ish columns (`status`, `source`, `media_kind`) are plain `TEXT`; parsing happens in
  `acgn-store/src/rows.rs` so bad data surfaces as a readable error instead of a panic.

## 4. Synchronization rules

This is the part future two-way sync (Bangumi, Xiaoheihe) depends on. It is enforced by
`crates/acgn-store/tests/import_flow.rs`; those tests are the specification.

| Data | Authority |
|---|---|
| Metadata (title, cover, price, review, description, …) | **Source** — overwrite when the source provides a value |
| Personal state (status, rating, note, progress, playtime, favorite) | **Local** — an import never overwrites it |
| Items that disappeared from the source | **Reported only** (`missing`), never deleted |
| Re-importing identical data | **No-op** (`unchanged`) — imports are idempotent |

Additional rules:

- New entries default to `wishlist`; JSON imports may request another status.
- `SourceItem.status` is adopted only while the local entry is still `wishlist` — importing an
  "owned" list moves a fresh entry to `backlog`, but never reverts `playing` / `completed`.
- Title comparison ignores `™` / `®` / case / repeated whitespace, so cosmetic renames do not
  count as changes.
- Every run is recorded, including failures (`import_runs.error`).
- `dry_run` (the "preview only" switch) computes the *same* diff as a real import through the
  shared `plan_merge` function, writes nothing and leaves no run row.

### 4.1 Metadata levels: list vs full

Sources return two kinds of payloads:

- **List level** (`MetadataLevel::List`): what a list endpoint gives you — name, review score,
  release string, a **generic** cover URL (…/apps/{id}/header.jpg).
- **Full level** (`MetadataLevel::Full`): what a detail endpoint gives you — hashed cover URLs,
  localized titles and release strings, prices, descriptions, developers, platforms.

For the fields where only the detail endpoint is authoritative — `cover_url`, `poster_url`,
`release_raw`, `release_date` and `metadata_json` — a **list-level item may only fill them when
empty**; overwriting requires a full-level item.

Without that rule, "import (list) → enrich (full) → import (list)" rewrites those fields back
and forth and every sync falsely reports `updated`, which also breaks the idempotency promise
above. `metadata_json` matters most here because the two payloads are *always* different.

### 4.2 Timestamps

SQLite keeps microseconds, sources may hand out nanoseconds. Instants are always compared at
microsecond precision (`timestamp_micros`), otherwise every sync sees a "change" that is really
just precision loss.

### 4.3 What never happens

- Deleting rows because a source stopped returning them.
- Overwriting user state (status / rating / note / progress / playtime / favorite).
- Writing anything during a dry run.
- Touching `games.updated_at` / `metadata_updated_at` when a refresh finds no changes (the
  frontend shows "metadata fetched … ago", so silent writes look like content changes).

## 5. Data sources (Steam today)

Three paths, chosen automatically:

| Path | Requires | Gives |
|---|---|---|
| Public `wishlistdata?p=N` JSON | public profile | name, cover, review, release date, added date, wishlist rank |
| Official `IWishlistService/GetWishlist/v1` | `STEAM_API_KEY` | appids only (titles need enrichment); supports `cursor` paging |
| Offline fixture (`ACGN_STEAM_FIXTURE`) | nothing | a frozen copy of the public data, used for demos and tests |

- The public path is preferred because it needs no key and returns richer data; the official
  API is a fallback when the profile is not public.
- `appdetails` (store details) is rate limited: enrichment runs with concurrency 2, a 400 ms
  gap, and stops gracefully on HTTP 429 — already-fetched items stay committed and the rest are
  retried later. A rate limit is surfaced as "try again later" (422), never as "app not found".
- `reqwest` errors are redacted with `without_url()` before they reach logs, `import_runs` or
  API responses: the official URL carries `?key=…`.
- From mainland China, both `store.steampowered.com` and `api.steampowered.com` need a proxy.
  A `Steam → DIRECT` rule in the proxy configuration is the usual cause of timeouts.

## 6. Frontend

- Vue 3 + TypeScript + Pinia + vue-router; no UI framework (the design system is
  `src/style.css`, dark-first).
- `src/api/` is the only place that knows about HTTP: `http.ts` (fetch wrapper + error class),
  `types.ts` (mirrors the Rust DTOs) and `index.ts` (typed endpoint functions).
- Pinia stores: `library` (list state, filters, in-place updates) and `meta` (backend config,
  status enum).
- Images come from Steam's CDN; a failed load falls back to a generated placeholder with the
  title, because the CDN is not reachable from every network.

## 7. Testing

- `acgn-store` integration tests run against an in-memory SQLite (`Db::memory()`) and assert
  the synchronization rules in §4 — including the level rules, dry-run parity, missing
  handling and search escaping.
- `acgn-steam` tests parse embedded JSON samples (never the network) and cover the ID parser,
  the release-date/count parsers, error redaction and fixture loading.
- `acgn-server` tests build the router with `tower::ServiceExt::oneshot` and assert the error
  envelope contract.
- `fixtures/steam/*.json` is **generated** by `scripts/gen-steam-fixture.mjs` from public
  endpoints; never hand-edit it. It is marked `linguist-generated` in `.gitattributes`.

## 8. Extension points

- **New source** (Bangumi, Xiaoheihe, …): write an adapter that produces `SourceItem`s and call
  `Db::import_items`; nothing in the merge logic may branch on a specific source. Statuses and
  media kinds are already shared, so anime/comic/novel reuse the same tables.
- **Tauri**: reuse `acgn-core` / `acgn-store` / `acgn-steam`, either by embedding `acgn-server`
  in-process (library target) or by pointing the same frontend at a local HTTP port.
- **Multi-device sync**: `import_runs` plus `updated_at` already provide the merge inputs;
  the open question is conflict policy, which will follow the same "local state wins" rule.
