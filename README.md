# ACGN Tools

[![Contributors](https://img.shields.io/github/contributors/bgzolab/acgn-tools.svg?style=for-the-badge)](https://github.com/bgzolab/acgn-tools/graphs/contributors)
[![Forks](https://img.shields.io/github/forks/bgzolab/acgn-tools.svg?style=for-the-badge)](https://github.com/bgzolab/acgn-tools/network/members)
[![Stargazers](https://img.shields.io/github/stars/bgzolab/acgn-tools.svg?style=for-the-badge)](https://github.com/bgzolab/acgn-tools/stargazers)
[![Issues](https://img.shields.io/github/issues/bgzolab/acgn-tools.svg?style=for-the-badge)](https://github.com/bgzolab/acgn-tools/issues)
[![Licence](https://img.shields.io/github/license/bgzolab/acgn-tools.svg?style=for-the-badge)](https://github.com/bgzolab/acgn-tools/blob/main/LICENCE)
[![Telegram](https://img.shields.io/badge/-telegram-black.svg?style=for-the-badge&logo=telegram&colorB=555)](https://t.me/imbGZo)

A self-hosted collection manager for **ACGN** (anime / comic / game / novel).
The current focus is **games**: import your Steam wishlist and owned-games library into your own
library, enrich it with store metadata, and keep track of what you actually play.

> Status: early MVP — the REST API and the web UI are usable; the Tauri desktop client and
> the non-game media types are not implemented yet.

## Features

- **Steam wishlist import in one click** — steamID64, vanity URL or profile link; no API key
  and no cookie required
- **Steam library sync** — owned games land in your backlog together with their cumulative
  playtime; needs `STEAM_API_KEY` (Steam's `GetOwnedGames` endpoint refuses anonymous calls)
  and a public "Game details" privacy setting
- **Metadata enrichment** — description, developer, publisher, genres, platforms, price,
  discount, release date and the vertical library poster, fetched on demand
- **Personal tracking** — status (wishlist / backlog / playing / completed / on hold /
  dropped / ignored), rating, progress, playtime, notes and favorites. Steam playtime follows
  the source automatically until you edit it by hand — your own number always wins
- **Library view** — filter by status, search, nine sort orders, pagination; multi-select mode
  for batch deleting entries you no longer care about
- **Delete & cleanup** — remove entries one by one from the detail drawer or in bulk from the
  library; deletion is local, so re-importing the same source brings its items back
- **Dashboard** — status breakdown, wishlist value per currency, total playtime, recently
  added, currently playing and recent sync history
- **Import history** — every run records added / updated / skipped / missing counts, duration
  and warnings (failed runs included)
- **Offline mode** — the bundled `fixtures/steam` sample data runs the whole flow without
  network access; the test suite uses the same approach
- **Full export** — `GET /api/export/json` for backups, and the basis for future sync

### Sync semantics

- Metadata (title, cover, price, review, …) follows the source; your status, rating, notes,
  progress and favorites always win — re-syncing never reverts them.
- Steam playtime is tracked from the source and follows it automatically until you edit the
  number by hand; clearing your value resumes following
  ([design.md §4.5](docs/design.md#45-playtime-source-tracked-overridden-locally)).
- An owned game moves a wishlist entry to *backlog* once; a `playing`/`completed` status is
  never reverted.
- Items that disappear from a source are only reported (`missing`), never deleted;
  re-importing unchanged data reports `unchanged` and writes nothing.

## Quick start

```bash
# Requirements: Rust 1.85+, Node 20+, pnpm
git clone git@github.com:bgzolab/acgn-tools.git && cd acgn-tools

# 1. Backend — listens on 127.0.0.1:8787, SQLite file at ./data/acgn.db
cargo run -p acgn-server -- serve

# 2. Frontend — the dev server proxies /api to the backend
cd web && pnpm install && pnpm dev     # → http://127.0.0.1:5173
```

Open the app, switch to **Import & Sync**, paste your SteamID64 (or vanity URL), and import.
Library sync additionally needs a Steam Web API key (`STEAM_API_KEY`, see below) and a public
"Game details" privacy setting.

Want to look around without touching the network first?

```bash
ACGN_STEAM_FIXTURE=fixtures/steam cargo run -p acgn-server -- serve
```

Deploy as a single binary (API and the built frontend on one port):

```bash
cargo build --release -p acgn-server
cd web && pnpm build
./target/release/acgn-server serve
```

## Configuration

Everything is configured through environment variables — see [`.env.example`](.env.example)
for a template.

| Variable | Default | Description |
|---|---|---|
| `ACGN_BIND` | `127.0.0.1:8787` | Listen address; use `0.0.0.0:8787` to expose it on your LAN |
| `ACGN_DB` | `./data/acgn.db` | SQLite file path |
| `ACGN_WEB_DIR` | `./web/dist` | Built frontend served by the backend |
| `ACGN_CORS_ORIGINS` | localhost:5173 only | Comma-separated CORS allowlist (same-origin deployments need none) |
| `ACGN_LOG` | `info,…` | `tracing` filter directives |
| `ACGN_STEAM_FIXTURE` | unset | Point at `fixtures/steam` to run fully offline |
| `STEAM_API_KEY` | unset | Optional for the wishlist (the official wishlist API works anonymously); **required for the library sync** — Steam's `GetOwnedGames` refuses anonymous calls |
| `STEAM_COUNTRY` / `STEAM_LANGUAGE` | `cn` / `schinese` | Store country and language (prices, titles, descriptions) |
| `STEAM_ENRICH_CONCURRENCY` / `STEAM_ENRICH_DELAY_MS` | `1` / `1500` | Metadata enrichment rate (≈0.67 req/s by default, matching Steam's `appdetails` budget) |

> Steam store and API endpoints are unreachable from mainland China without a proxy. If
> requests time out, check your proxy's routing rules first — a `Steam → DIRECT` rule is the
> usual culprit.

## Documentation

| Document | Contents |
|---|---|
| [`docs/design.md`](docs/design.md) | Architecture, data model and synchronization rules |
| [`docs/api.md`](docs/api.md) | HTTP API reference; runnable samples in [`docs/api.http`](docs/api.http) |
| [`AGENTS.md`](AGENTS.md) | Working agreements for AI coding agents |

## Development

```bash
just verify     # fmt check + clippy + tests + frontend typecheck and build
just demo       # run the offline demo
just fixture    # regenerate the bundled sample data (this one hits the network)
```

The quality gates are `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`
and `cd web && pnpm typecheck && pnpm build`.
The [`ci` workflow](.github/workflows/ci.yml) runs all of them on every pull request (including
drafts); `opencode-review` waits for that workflow to succeed and does not re-run the gates
itself. [`AGENTS.md`](AGENTS.md) lists every command and convention.

## Roadmap

- [x] Domain model, SQLite storage and import merge semantics
- [x] Steam wishlist + library (owned games with playtime) import, metadata enrichment, dashboard, import history
- [ ] Price history and discount alerts
- [ ] Bangumi integration (anime / comic / novel, two-way progress sync)
- [ ] Xiaoheihe (小黑盒) — no public API; needs research or a manual import path
- [ ] Tauri desktop client reusing the same crates and frontend
- [ ] Incremental multi-device sync

## Contributing

Contributions of any kind are **greatly appreciated**. If you have a suggestion, fork the repo
and open a pull request, or open an issue with the `enhancement` tag. And a star never hurts 🙂

1. Fork the project
2. Create your feature branch (`git checkout -b feature/AmazingFeature`)
3. Commit your changes — English `type(scope):subject` title (a Chinese body is fine), e.g.
   `feat(steam):support vanity URLs`
4. Push to the branch (`git push origin feature/AmazingFeature`)
5. Open a pull request — it will be merged with **squash**

Top contributors:

<a href="https://github.com/bgzolab/acgn-tools/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=bgzolab/acgn-tools" alt="contrib.rocks image" />
</a>

## License

All code is licensed under the AGPL-3.0 license. See [`LICENCE`](LICENCE).
