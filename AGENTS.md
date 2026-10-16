# AGENTS.md

Working agreements for AI coding agents (opencode, Cursor, Copilot, …) in this repository.
Human-facing documentation lives in [`README.md`](README.md); this file is the machine-oriented
summary of **how to change this codebase safely**.

## Docs map

| Document | Audience | Contents |
|---|---|---|
| [`README.md`](README.md) | users | what it is, quick start, configuration, roadmap |
| [`docs/design.md`](docs/design.md) | maintainers & agents | architecture, data model, **synchronization rules (§4 — the spec the tests encode)** |
| [`docs/api.md`](docs/api.md) | integrators | HTTP contract: error envelope, PATCH semantics, endpoint notes |
| [`docs/api.http`](docs/api.http) | anyone | runnable request samples (VS Code REST Client) |

Keep design rationale out of the README: user-facing instructions in `README.md`, design and
trade-offs in `docs/design.md`, working agreements in this file.

---

## 1. What this project is

`acgn-tools` is a self-hosted collection manager for **ACGN** (anime / comic / game / novel).
Current scope is **games**: import a Steam wishlist into a local SQLite library and manage
personal state (status, rating, progress, playtime, notes, favorite).

Planned next: Bangumi integration (bidirectional progress sync), Xiaoheihe (小黑盒), and a
Tauri desktop client. Backend and frontend are split on purpose so that both the Rust crates
and the Vue app can be reused by Tauri without rewrites.

Non-goals for now: multi-user accounts, server-side auth, a mobile app.

## 2. Repository layout

| Path | Responsibility |
|---|---|
| `crates/acgn-core` | Domain models, DTOs, shared contracts. **Zero I/O dependencies** (no `sqlx`, no `reqwest`, no `axum`). |
| `crates/acgn-store` | SQLite persistence (`sqlx`), migrations, queries/pagination, import merge, run history. |
| `crates/acgn-steam` | Steam data sources: public wishlist JSON → official Web API → offline fixture. |
| `crates/acgn-server` | `axum` REST API + static hosting of the built frontend. |
| `web/` | Vue 3 + Vite + TS + Pinia + vue-router SPA (talks to the API over `/api`). |
| `fixtures/steam/` | Generated sample data for the offline demo and parser tests. |
| `scripts/` | Maintenance scripts (fixture generator). |

Dependency direction is strictly `acgn-server` → {`acgn-store`, `acgn-steam`} → `acgn-core`.
Do not make `acgn-core` depend on outer crates, and do not add I/O crates to it.

## 3. Commands

```bash
# backend (default: 127.0.0.1:8787, SQLite at ./data/acgn.db)
cargo run -p acgn-server -- serve

# offline demo (uses the generated fixtures, no network at all)
ACGN_STEAM_FIXTURE=fixtures/steam cargo run -p acgn-server -- serve
ACGN_STEAM_FIXTURE=fixtures/steam cargo run -p acgn-server -- seed   # import once and exit
cargo run -p acgn-server -- stats                                    # print a DB summary

# frontend (Vite dev server proxies /api to 8787)
cd web && pnpm install && pnpm dev

# quality gates — all four must pass before you push
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd web && pnpm typecheck && pnpm build

# regenerate the offline fixtures (this one DOES hit the network)
node scripts/gen-steam-fixture.mjs
```

`just` shortcuts exist for the common flows: `just demo`, `just dev`, `just verify`, `just fixture`.

## 4. Invariants — do not break these

### 4.1 Sync semantics (the core contract)

| Data | Authority |
|---|---|
| Metadata (title, cover, price, review, description, …) | **Source** — overwrite when the source provides a value |
| Personal state (status, rating, note, progress, playtime, favorite) | **Local** — an import must never overwrite it |
| Items that disappeared from the source | **Reported only** (`missing`), never deleted |
| Re-importing identical data | Must be a no-op (`unchanged`), imports are idempotent |

`SourceItem.status` is only adopted when the local entry is still `wishlist` (e.g. importing an
"owned" list may move a fresh entry to `backlog`, but never reverts a user's `playing`/`completed`).

Any change to this table requires updating `crates/acgn-store/tests/import_flow.rs` **and** the
"Sync semantics" section of `README.md`. Those tests are the specification.

### 4.2 `MetadataLevel` (list vs full)

`SourceItem.metadata_level` is `List` (wishlist-level: generic cover URL, English release string)
or `Full` (store-detail level: hashed cover, localized strings, prices, description).

Cover, poster, `release_raw` and `release_date` are **detail-authoritative** fields: a `List` item
may only fill them when empty. Without this rule, "import → enrich → import again" rewrites those
fields back and forth and every sync falsely reports `updated`.

### 4.3 Timestamps

SQLite stores microseconds, sources may hand out nanoseconds. Always compare instants at microsecond
precision (`timestamp_micros`), otherwise every sync looks like a change.

### 4.4 `dry_run`

A dry run must not write anything: no `games`/`library_entries` rows and **no `import_runs` row**
(`summary.run_id == 0`, API returns `"run": null`). It only reports what would happen.

### 4.5 `SourceItem` is the only source contract

To add Bangumi, Xiaoheihe, or any other source: write an adapter that produces `SourceItem`s and
call `Db::import_items`. Do not add source-specific branches to the merge logic in `acgn-store`.

### 4.6 Storage rules

- Migrations live in `crates/acgn-store/migrations/` and are embedded via `sqlx::migrate!`.
  **Append new migrations; never edit an already applied one.**
- Use runtime queries (`sqlx::query_as::<_, Row>()`, `QueryBuilder`) with `#[derive(sqlx::FromRow)]`
  rows. Do **not** use the `query!`/`query_as!` macros: they require a live database at build time.
- Enum-ish columns (`status`, `source`, `media_kind`) are plain `TEXT`; conversion happens in
  `crates/acgn-store/src/rows.rs`, so parsing failures surface as readable errors.
- Array-ish columns (`developers`, `publishers`, …) are JSON strings.

### 4.7 API rules

- Everything lives under `/api`; JSON is `snake_case`; the API index is `GET /api`.
- Errors are `{"error": {"kind", "message", "hint"}}` with meaningful HTTP codes
  (`400` invalid, `404` not found, `422` source rejected, `502` upstream/network, `500` storage).
- `PATCH /api/games/{id}`: missing field = unchanged; negative number = clear
  (`rating`/`progress_percent`/`playtime_minutes`); empty `note` = clear.
- Status keys are exactly: `wishlist`, `backlog`, `playing`, `completed`, `on_hold`, `dropped`, `ignored`.
- The server binds to `127.0.0.1` by default and only allows the origins listed in
  `ACGN_CORS_ORIGINS`. Do not loosen either one without an explicit security rationale.

### 4.8 Tests never touch the network

`crates/acgn-steam` tests parse embedded sample JSON; `crates/acgn-store` tests run against an
in-memory SQLite (`Db::memory()`), with no fixtures on disk. When you fix a bug, add the failing
case as a test first.

## 5. Conventions

- **Commit titles in English**, `type(scope):subject` (e.g. `feat(steam):support vanity URLs`).
  The body may be Chinese and should explain what/why, not just restate the diff.
- Branches: `feat/…`, `fix/…`, `chore/…`. Commit freely on the branch; the PR is merged with
  **squash**, so the branch history is not part of `main`.
- PRs are reviewed by the `opencode-review` workflow; it only runs when the PR author is `bgzo`
  and the PR is not a draft.
- Keep code comments in the style already used (Chinese comments are fine); user-facing UI strings
  are Simplified Chinese.
- Generated files (`fixtures/steam/*.json`, `Cargo.lock`, `web/pnpm-lock.yaml`) are marked
  `linguist-generated` in `.gitattributes`. Never hand-edit `fixtures/steam/`; regenerate it.
- Do not commit `data/*.db`, `web/dist/`, `target/`, or `.env`.

## 6. Environment and operations

| Variable | Purpose |
|---|---|
| `ACGN_BIND` | Listen address (default `127.0.0.1:8787`) |
| `ACGN_DB` | SQLite path (default `./data/acgn.db`) |
| `ACGN_WEB_DIR` | Frontend build output served by the API (default `./web/dist`) |
| `ACGN_CORS_ORIGINS` | Comma-separated CORS allowlist |
| `ACGN_STEAM_FIXTURE` | Point at `fixtures/steam` to run fully offline |
| `STEAM_API_KEY` | Optional; only sent to the official API when configured (that endpoint works anonymously) |
| `STEAM_COUNTRY` / `STEAM_LANGUAGE` | Store country/language for prices and names |
| `STEAM_ENRICH_CONCURRENCY` / `STEAM_ENRICH_DELAY_MS` | Enrichment rate (default 1 / 1500ms ≈ 0.67 req/s, the `appdetails` budget) |

Operational notes:

- Steam endpoints (`store.steampowered.com`, `api.steampowered.com`) are unreachable without a
  proxy from mainland China. If a request times out, check the proxy's **rule routing** first:
  a `Steam → DIRECT` rule is the usual culprit.
- `appdetails` is rate limited; enrichment runs with concurrency 2 + a 400 ms gap and stops
  gracefully on HTTP 429, leaving already-fetched items committed.
- The review CI needs the `OPENCODE_API_KEY` secret on the repository (or an organization secret
  granted to it). Without it the provider cannot be resolved and the run fails with
  `Model not found: opencode-go/glm-5.3-flash`. Organization secrets are **not** inherited
  automatically: add the repository under *Repository access* (or switch it to *All repositories*).
- Private repositories additionally need credentials for the handler's
  `git fetch origin --depth=20 <head>`; the review workflow already injects a read-only
  `GITHUB_TOKEN` via a global `url.insteadOf` rewrite. Removing that step breaks private repos
  with `fatal: could not read Username for 'https://github.com'`.
- The review CI runs the scripts in `.github/scripts/`, so the PR's **head branch must contain them**
  (they are shipped by `main`; keep both in sync when upgrading the pipeline).

## 7. Definition of done

A change is ready when all of the following hold:

1. `cargo fmt`, `cargo clippy … -D warnings`, `cargo test --workspace`, `pnpm typecheck && pnpm build` pass.
2. New behaviour is covered by a test, or the PR explains why it cannot be.
3. Sync semantics / API / env-var docs are updated when they changed (`README.md`, `docs/api.http`, this file).
4. No secrets, databases, build output, or hand-edited fixtures are committed.
