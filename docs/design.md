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
| `library_entries` | My relationship to a game: status, rating, progress, playtime, achievements, note, favorite, plus sync bookkeeping. 1:1 with `games` today |
| `accounts` | A **source identity** (SteamID64): persona name, avatar, profile URL, region, raw profile payload, last sync. Unique per `(source, external_id)`. Not a multi-user login |
| `game_accounts` | Many-to-many "this game is on that account": wishlist membership, per-account playtime / last played, and the account-region price |
| `import_runs` | One row per import / enrichment run: counts, warnings, duration, error |

Rationale:

- Splitting *catalog* from *my state* keeps the merge rules simple and testable, and leaves
  room for future `holdings` (the same game owned on several platforms) without touching
  either core model.
- Account association is a separate join table rather than a column on `library_entries`:
  the same game can be owned by several accounts, each with its own playtime and regional
  price, while personal state (rating / note / status) stays single.
- `accounts.metadata_json` stores the raw `GetPlayerSummaries` player object (or the
  community-XML fallback): "however many fields the API returns" are kept, so future UI can
  show online state, creation date, … without a migration.
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
| Personal state (status, rating, note, progress, favorite) | **Local** — an import never overwrites it |
| Playtime | **Source-tracked, locally overridable** — imports record the source value and `playtime_minutes` follows it until the user edits it by hand (see §4.5) |
| Account association (owned / wishlist membership, per-account playtime) | **Source** — an item belongs to the account the sync was run for |
| Items that disappeared from the source | **Reported only** (`missing`), never deleted; scoped per account (§4.6) |
| Re-importing identical data | **No-op** (`unchanged`) — imports are idempotent |

Additional rules:

- New entries default to `wishlist`; JSON imports may request another status.
- `SourceItem.status` is adopted while the local entry is still `wishlist` or `backlog` (the
  "not user-confirmed" tiers), never demoting to `wishlist`. The Steam library uses it to
  promote a game with `playtime_forever > 0` to `playing` (never-played → `backlog`); a status
  the user set (`playing` / `completed` / `on_hold` / `dropped` / `ignored`) is never reverted.
- The achievement check (`Db::update_achievements`) uses the same tier idea: a game with all
  achievements (`total > 0 && achieved >= total`) promotes `wishlist` / `backlog` / `playing`
  to `completed` (filling `finished_at` / 100% progress when empty). User-set states are never
  reverted, and failed checks keep the last known progress.
- `missing` candidates differ per import kind (`MissingScope`): a wishlist import only reports
  entries that were on the wishlist last time, a library import only reports entries that were
  seen in a previous library sync (`source_playtime_minutes IS NOT NULL`). Mixing the two would
  flag every unowned wishlist entry as "gone" during a library sync. With an account
  (`ImportRequest.account_id`) the candidates come from `game_accounts` for **that account**
  (`in_wishlist = 1` / `source_playtime_minutes IS NOT NULL`), so one account's refund never
  shows up as another account's missing item (§4.6).
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

### 4.3 Two write paths: sync vs restore

`import_items` (the merge used by every source adapter) follows §4.1. **Restoring a backup is a
different operation with different rules**: `Db::restore_items` / `POST /api/imports/restore`
treat the file as authoritative — metadata *and* personal state come from it, and nothing is
deleted. Bookkeeping timestamps are refreshed. Keep them separate; do not route a restore through
the merge, or restored ratings/notes will be silently dropped.

### 4.4 What never happens

- Deleting rows because a source stopped returning them.
- Overwriting user state (status / rating / note / progress / favorite — and a playtime the user
  edited by hand; see §4.5).
- Writing anything during a dry run.
- Touching `games.updated_at` / `metadata_updated_at` when a refresh finds no changes (the
  frontend shows "metadata fetched … ago", so silent writes look like content changes).

The only way rows leave the database is an explicit user delete (`DELETE /api/games/{id}` /
`POST /api/games/delete`, see [`api.md`](api.md)). That is a local, one-sided action: it cascades
to `library_entries` and is not recorded in `import_runs`. An item that still exists in the source
is re-created by the next import (as `added`) — that is intentional. To keep something out of the
way without losing it, use the `ignored` status instead of deleting.

### 4.5 Playtime: source-tracked, overridden locally

Steam is the authority for *playtime as a measurement*, but the user may have data Steam does not
know (console / offline hours). Both are kept:

- `library_entries.source_playtime_minutes` — the value the last import reported (0 counts as
  seen; it also marks "this entry was part of a library sync" for `MissingScope::Library`);
- `library_entries.playtime_minutes` — the effective value used everywhere (list sorting,
  dashboard totals, detail view). It **follows the source while it is empty or still equal to the
  tracked source value**, and stops following as soon as the user edits it (`PATCH`).

Clearing via `PATCH` (`playtime_minutes: -1`) means "drop my manual value": the effective value
reverts to the tracked source value when one exists, otherwise it becomes empty. Adopting the
current Steam value is just a `PATCH` with that number — equality with the track resumes
automatic following. Imports never write `playtime_minutes` when the source has no playtime
(wishlist items), so a library sync followed by a wishlist sync cannot wipe the tracked value.

With several accounts owning the same game, `source_playtime_minutes` is the **maximum** of the
per-account values in `game_accounts` (§4.6). A low-playtime alt therefore never overwrites the
main account's hours, and the "follows the source" comparison uses the aggregate value.

### 4.6 Multiple source accounts

A user may have several Steam accounts (main, alt, a foreign-region account). Accounts are
stored once per `(source, external_id)` and every Steam import runs **for one account**:

- `POST /api/imports/steam/{wishlist,library,achievements}` takes `account_id` (or a raw
  `steam_id`, which creates the account on the fly). The same request resolves / refreshes the
  profile (name, avatar, region) and records run args with the account label.
- Each accepted item upserts a `game_accounts` row for `(game, account)`:
  `in_wishlist` for wishlist imports, `source_playtime_minutes` / `playtime_2weeks_minutes` /
  `last_played_at` for library imports. A library hit clears `in_wishlist` for that account
  (Steam removes owned games from the wishlist); other accounts' rows are untouched.
- `missing` is computed against the **same account's** previous associations (§4), never the
  whole database. Items keep their rows when they disappear — the next sync reports them again
  until the source returns or the user deletes them.
- The entry-level `source_playtime_minutes` is the `MAX` across that game's accounts. It is
  updated before `plan_merge`, so dry-run and real imports report the same diff.
- Associating an existing game for a new account counts as `updated`; re-importing the exact
  same associations is `unchanged`. The account link's `last_synced_at` is bookkeeping and does
  not count as a change.
- `accounts.last_synced_at` is refreshed after a successful import (entries and achievements),
  but not by a dry run.
- Databases created before this feature have no associations: games stay visible and keep their
  personal state, but they only appear under an account filter after the next sync of that
  account. The first sync of each account starts with an empty `missing` scope; the entry-level
  aggregate reaches the real maximum once every owning account has synced.
- Deleting an account (`DELETE /api/accounts/{id}`) cascades only `game_accounts`; games and
  personal state are kept, and the next sync recreates the associations.
- Generic JSON imports (`POST /api/imports`) and restores do **not** create account links;
  backups deliberately contain metadata + personal state only (§4.3).

### 4.7 Region prices

Steam prices depend on the store region. Each account has an effective region
(`country_override` ?? profile `loccountrycode`) and `game_accounts` stores that account's price:

- `POST /api/enrich` runs the normal metadata pass (configured `STEAM_COUNTRY`) and then a
  price pass for associations whose region differs from the configured one and whose
  `price_updated_at IS NULL`. Requests are grouped by country and appid, so two accounts in the
  same region share one fetch.
- `price_updated_at` means "this region was asked": no currency/final price with a timestamp is
  the honest "not sold in this region", not "not fetched". The UI only falls back to the global
  `games.price_*` when `price_updated_at` is still empty.
- Accounts whose effective region equals `STEAM_COUNTRY` are not fetched separately (the global
  price is the same region), and accounts without any region fall back to the global price too.
- Changing an account's region clears its cached prices (`price_*`, `price_updated_at`) so the
  next enrichment re-fetches them instead of showing the old region's currency.
- The same appdetails rate limit applies; a 429 stops the pass gracefully and leaves un-fetched
  associations pending for the next click.

## 5. Data sources (Steam today)

Three paths, chosen automatically:

| Path | Requires | Gives |
|---|---|---|
| Public `wishlistdata?p=N` JSON | public profile | name, cover, review, release date, added date, wishlist rank |
| Official `IWishlistService/GetWishlist/v1` | nothing (a key is optional and only sent when configured) | appid, priority, date_added; supports `cursor` paging |
| Store catalog `IStoreBrowseService/GetItems/v1` | nothing (anonymous, 50 ids per request) | localized **name**, item type, vertical `library_capsule` |
| Offline fixture (`ACGN_STEAM_FIXTURE`) | nothing | a frozen copy of the public data, used for demos and tests |

- The public path is tried first because it returns richer data (names, covers, review scores).
  Steam has tightened anonymous access to it (as of 2026-10 it answers `302` to the store home
  for everyone), so **the official path is the one that usually delivers** — and it needs neither
  a key nor a cookie.
- Because the official path only returns appids, the wishlist importer immediately batch-fills
  titles, item types and posters from the store catalog (50 appids per request, with the same
  `us` retry for region-hidden items). Only appids the catalog does not return (delisted /
  invisible) keep the `Steam App <id>` placeholder; prices and descriptions still come from
  enrichment.
- Wishlist pagination is paced at 250 ms per page (and the one-click sync runs the steps
  serially), so a large wishlist cannot hammer the source in one burst.
- `appdetails` (store details) is rate limited to roughly **200 requests per 5 minutes**, so
  enrichment defaults to concurrency 1 with a 1500 ms gap (≈0.67 req/s) and stops gracefully on
  HTTP 429 — already-fetched items stay committed and the rest are retried later. A rate limit
  is surfaced as "try again later" (422), never as "app not found".
- Store details are region-sensitive: titles that are not sold in the configured country return
  `success:false` for `cc=cn`, so the client retries once with `cc=us`. Those entries then carry
  USD prices, which is the honest answer ("not sold in your region").
- Vertical posters (`library_capsule`, 600x900) come from `IStoreBrowseService/GetItems/v1`
  (anonymous, batched, 50 ids per request). `appdetails` only carries landscape headers, and the
  legacy `.../steam/apps/{id}/library_600x900.jpg` path 404s for some titles (adult /
  region-restricted content in practice). When the configured country hides a game's assets
  entirely (adult content under `cc=cn`), the client retries that chunk with `cc=us`. Poster
  failures are logged, never surfaced as enrichment errors, and never overwrite a previously
  stored poster: without a fresh asset URL the incoming value is `None`, which the merge treats
  as "keep the local value".
- `reqwest` errors are redacted with `without_url()` before they reach logs, `import_runs` or
  API responses: the official URL carries `?key=…`.
- From mainland China, both `store.steampowered.com` and `api.steampowered.com` need a proxy.
  A `Steam → DIRECT` rule in the proxy configuration is the usual cause of timeouts.

### 5.1 Library sync (owned games + playtime)

| Path | Requires | Gives |
|---|---|---|
| Official `IPlayerService/GetOwnedGames/v1` | `STEAM_API_KEY` + a public "Game details" privacy setting | every owned app in one request: appid, name, `playtime_forever` (minutes), `playtime_2weeks`, `rtime_last_played` |
| Offline fixture (`library.json`) | nothing | the frozen `GetOwnedGames` payload used by the demo and tests |

- Unlike the wishlist endpoints, `GetOwnedGames` **refuses anonymous calls** (verified 2026-10:
  HTTP 401 `Please verify your key= parameter`), and the community XML game list
  (`games?tab=all&xml=1`) now redirects anonymous visitors to the login page, so a key is the
  only supported path. The UI hides the library buttons when
  `GET /api/config` reports `steam.has_api_key: false` (fixture mode excepted).
- Privacy is the only other gate: with private game details Steam answers `{"response": {}}`.
  That is reported as "make Game details public", never confused with an empty library
  (`{"response": {"game_count": 0, "games": []}}`).
- Owned items arrive as list-level metadata. A game with `playtime_forever > 0` is imported as
  `playing`, a never-played one as `backlog`; both `wishlist` and `backlog` entries are upgraded
  through the regular status-adoption rule, while a status the user set is never reverted.
  Playtime is carried in `SourceItem.playtime_minutes` and merged per §4.5.
- One request covers the whole library, so there is no pagination or rate-limit dance; the
  expensive part remains metadata enrichment.
- Playtime caveat (verified 2026-10): Steam returns `playtime_forever: 0` for every owned game
  when the account enabled *"Always keep my total playtime private"* (a checkbox under the
  Game-details privacy setting); ownership is still listed normally. `playtime_2weeks` /
  `rtime_last_played` are only returned when the queried profile is the API key's owner —
  they are optional everywhere in the client.
- The fixture generator writes `library.json` with deterministic demo playtimes
  (`--library-only` regenerates it from the existing `wishlist.json` without touching the
  network). Real playtimes need a real key and profile.

### 5.2 Achievement check (all achievements → completed)

- `ISteamUserStats/GetPlayerAchievements` is **per game**. There is no usable bulk endpoint:
  `IPlayerService/GetAchievementsProgress` answers `401` for every request form we tried
  (GET → 405, GET-style POST → 411/401, JSON body → 401). So the check is a separate on-demand
  action (`POST /api/imports/steam/achievements`, run mode `steam_achievements`), not part of
  every library sync.
- The response shapes are distinguished by payload, not by HTTP status (400/403 carry a JSON
  answer): `success: true` + achievements (progress), `success: true` without the array (the
  game has no achievement set → `0/0`), `error: "Requested app has no stats"` (HTTP 400 → also
  `0/0`), and `error: "Profile is not public"` (HTTP 403 — not owned or hidden → no data). A
  known key-failure message maps to a rejected error (422); anything unknown maps to an
  upstream error (502), so one weird payload cannot silently spin the whole poll. Unavailable
  entries keep their previous progress and still advance the polling clock, so they cannot
  block the queue.
- Poll order is `achievements_checked_at` null-first, then oldest-first (rotating refreshes),
  and the response reports `pending` (never checked) so the UI can offer "click again to
  continue". Checking is paced at 2 concurrent + a 250 ms gap (~8 games/s); HTTP 429 stops the
  run gracefully and already-processed entries stay committed. Five consecutive request
  failures stop the run early (systemic failure protection: bad key returning JSON, upstream
  changes, …), and every run — including a stopped or failed one — is recorded in
  `import_runs`.
- A check that finds nothing new only advances `achievements_checked_at`; `updated_at` moves
  only when stored values actually changed (§4.4), and a single row failing to write (for
  example an entry deleted mid-run) never aborts the batch.
- Full achievements promote `wishlist` / `backlog` / `playing` to `completed`; user-set
  `completed` / `on_hold` / `dropped` / `ignored` are never reverted.
- The fixture generator writes deterministic demo achievements (`achievements.json`) so the
  offline demo exercises the same path.

### 5.3 Account profiles

Accounts are created on demand: `POST /api/accounts/resolve` (and any sync that receives a raw
`steam_id`) resolves the input, creates the row if needed and refreshes the profile.

| Path | Requires | Gives |
|---|---|---|
| `ISteamUser/GetPlayerSummaries/v2` | `STEAM_API_KEY` | persona name, avatar URLs, profile URL, `loccountrycode`, online state, creation date, … (the whole player object is stored in `metadata_json`) |
| Community XML `profiles/{id}/?xml=1` | nothing | persona name, avatar, custom URL / profile URL; no region |
| Offline fixture (`profile.json`) | nothing | a frozen player object for the demo and tests |

- Profile refreshes are best-effort: a failure is a warning and never blocks a wishlist / library
  sync. The same profile fetched twice does not move `updated_at` (§4.4).
- `loccountrycode` is the profile's location, not necessarily the account's purchase region, so
  `country_override` can be set manually (`PATCH /api/accounts/{id}`); an empty value restores
  following the profile. The effective region is `COALESCE(country_override, country_code)`.
- Deleting an account is local: associations cascade, games and personal state stay.

## 6. Frontend

- Vue 3 + TypeScript + Pinia + vue-router; no UI framework (the design system is
  `src/style.css`, dark-first).
- `src/api/` is the only place that knows about HTTP: `http.ts` (fetch wrapper + error class),
  `types.ts` (mirrors the Rust DTOs) and `index.ts` (typed endpoint functions).
- Pinia stores: `library` (list state, filters, in-place updates), `accounts` (account list used
  by the import page and the library filter) and `meta` (backend config, status enum).
- The import page is account-first: add an account once, then select a card and use the single
  **one-click sync** button. It runs wishlist → library → achievements serially and shows a step
  list + progress bar (achievements continue in 200-game batches until no never-checked entries
  remain; network/upstream errors retry once; a failed step does not abort the rest). "Preview"
  dry-runs the two imports without checking achievements.
- The library filters by account and cards show the selected account's regional price plus
  account avatars; the detail drawer lists every linked account with its own playtime and price.
- Images come from Steam's CDN; a failed load falls back to a generated placeholder with the
  title, because the CDN is not reachable from every network. Account avatars fall back to a
  monogram on load failure (the fixture uses an inline SVG avatar, so the offline demo works).

## 7. Testing

- `acgn-store` integration tests run against an in-memory SQLite (`Db::memory()`) and assert
  the synchronization rules in §4 — including the level rules, dry-run parity, missing
  handling, playtime provenance, achievement promotion and search escaping.
- `crates/acgn-store/tests/accounts_flow.rs` is the account specification: idempotent account
  upsert, per-account wishlist / library associations, per-account `missing`, the `MAX`
  playtime aggregate, account filtering / link attachment, regional prices and cache clearing,
  and dry-run parity.
- `acgn-steam` tests parse embedded JSON samples (never the network) and cover the ID parser,
  the release-date/count parsers, profile parsing (summaries + XML), error redaction and fixture
  loading.
- `acgn-server` tests build the router with `tower::ServiceExt::oneshot` and assert the error
  envelope contract plus the account endpoints (resolve / list / region patch / delete,
  account-filtered games, regional enrichment) against a temporary fixture directory.
- `fixtures/steam/*.json` is **generated** by `scripts/gen-steam-fixture.mjs` from public
  endpoints; never hand-edit it. `profile.json` can be regenerated offline with
  `node scripts/gen-steam-fixture.mjs --profile-only`. The directory is marked
  `linguist-generated` in `.gitattributes`.

## 8. Extension points

- **New source** (Bangumi, Xiaoheihe, …): write an adapter that produces `SourceItem`s and call
  `Db::import_items`; nothing in the merge logic may branch on a specific source. Statuses and
  media kinds are already shared, so anime/comic/novel reuse the same tables. `accounts` and
  `game_accounts` carry a `source` column, so a future Bangumi account can reuse the same
  profile / association model without a schema change.
- **Tauri**: reuse `acgn-core` / `acgn-store` / `acgn-steam`, either by embedding `acgn-server`
  in-process (library target) or by pointing the same frontend at a local HTTP port.
- **Multi-device sync**: `import_runs` plus `updated_at` already provide the merge inputs;
  the open question is conflict policy, which will follow the same "local state wins" rule.
