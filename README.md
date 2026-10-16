# ACGN Tools

自建的 **ACGN（Anime / Comic / Game / Novel）收藏管理工具**，当前聚焦 **Game**：
先把 Steam 愿望单完整搬进自己的库，再逐步接入 Bangumi、小黑盒等社区，做进度双向同步。

> 状态：**MVP / 脚手架阶段**（后端 REST API 已可用，前端为 Vue 3 SPA，Tauri 尚未接入）

---

## 现在能做什么

- **一键导入 Steam 愿望单**：输入 steamID64 / 个性域名 / 主页链接即可，走公开接口，**不需要 API Key**
- **元数据补全**：按需抓取商店详情（简介、开发商、发行商、类型、平台、价格、折扣、发售日）
- **个人进度管理**：状态（愿望单 / 待玩 / 在玩 / 已通关 / 搁置 / 弃坑 / 忽略）、评分、进度百分比、时长、备注、收藏
- **筛选 / 搜索 / 排序 / 分页**：好评优先、愿望单排位、价格、最近加入……
- **看板**：条目分布、愿望单参考价值（按币种）、最近加入、在玩、同步历史
- **导入历史留档**：每次导入 / 补全都记录新增、更新、跳过、消失、耗时与告警
- **离线模式**：`fixtures/steam` 里的真实样例数据，完全断网也能跑通全流程（演示 / 回归测试）
- **全量导出**：`GET /api/export/json`，可用于备份，也是未来多端同步的基础

## 工作方式（同步语义）

这是设计里最关键的一条，决定了未来「双向同步」会不会打架：

| 数据 | 谁说了算 |
|---|---|
| 元数据（标题、封面、价格、评价、简介……） | **来源（Steam / Bangumi）**，来源给了值就覆盖 |
| 个人状态（状态、评分、备注、进度、时长、收藏） | **本地**，导入永远不覆盖 |
| 来源里消失的条目 | **只提示不删除**（可能只是买了 / 移出愿望单） |
| 一切操作 | 幂等，重复导入只会「未变」 |

补充规则：

- 新建条目时的初始状态默认是「愿望单」，可在 JSON 导入里用 `status` 指定；
- 只有条目**还停留在愿望单**时，来源的状态才会被采纳（例如从「已拥有」导入时会自动变成待玩）；
- 标题比较会忽略 `™` / `®` / 大小写差异，避免每次同步都判定为「有变化」；
- 每次运行都写 `import_runs`，失败也留档（前端「同步历史」可展开看告警）。

## 架构

```
                    ┌──────────────────────────────────────────┐
   浏览器 / Tauri ──▶│            web（Vue 3 + Vite）            │
                    └───────────────┬──────────────────────────┘
                                    │ HTTP /api（前后端分离）
                    ┌───────────────▼──────────────────────────┐
                    │   acgn-server（axum）：REST + 静态托管     │
                    └───────┬───────────────────────┬──────────┘
                            │                       │
              ┌─────────────▼──────────┐   ┌────────▼───────────────┐
              │ acgn-store（SQLite）   │   │ acgn-steam（数据源适配）│
              │ 仓储 + 导入合并逻辑     │   │ 公开 JSON / 官方 API    │
              └─────────────┬──────────┘   └────────┬───────────────┘
                            │                       │
                    ┌───────▼───────────────────────▼──────────┐
                    │      acgn-core：领域模型 + SourceItem     │
                    └──────────────────────────────────────────┘
```

- `acgn-core`：纯领域模型（不依赖 sqlx / reqwest / axum）。**接入新社区的唯一契约是 `SourceItem`**。
- `acgn-store`：SQLite（sqlx + 迁移）、分页查询、导入合并、运行记录。未来 Tauri 桌面端可直接复用同一个库文件。
- `acgn-steam`：三条取数路径（公开 `wishlistdata` → 官方 `IWishlistService` → 离线 fixture），统一产出 `SourceItem`。
- `acgn-server`：axum REST API，存在 `web/dist` 时同端口托管前端（单二进制部署）。
- `web`：Vue 3 + TS + Pinia + vue-router，**Tauri 未来直接复用这一套**。

## 快速开始

```bash
# 0. 依赖：Rust 1.85+ / Node 20+ / pnpm
git clone git@github.com:bgzolab/acgn-tools.git && cd acgn-tools

# 1. 后端（默认只监听 127.0.0.1:8787，SQLite 落在 ./data/acgn.db）
cargo run -p acgn-server -- serve

# 2. 前端（开发模式，Vite 会把 /api 代理到 8787）
cd web && pnpm install && pnpm dev   # → http://127.0.0.1:5173
```

只想先看看界面效果（不联网）：

```bash
# 用仓库里的真实样例数据跑（fixtures/steam）
ACGN_STEAM_FIXTURE=fixtures/steam cargo run -p acgn-server -- serve

# 或者：导入一次样例愿望单后退出
ACGN_STEAM_FIXTURE=fixtures/steam cargo run -p acgn-server -- seed
cargo run -p acgn-server -- stats
```

生产 / 服务器部署：

```bash
cargo build --release -p acgn-server      # target/release/acgn-server
cd web && pnpm build                      # → web/dist
./target/release/acgn-server serve        # 同时托管 API 与前端
```

## HTTP API

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api` | 接口索引（自描述） |
| GET | `/api/health` | 健康检查 |
| GET | `/api/config` | 前端配置（状态枚举、数据源模式） |
| GET | `/api/stats` | 看板统计 |
| GET | `/api/games` | 列表：`status` `source` `q` `sort` `favorite` `page` `per_page` |
| GET | `/api/games/{id}` | 详情（`game` + `entry`） |
| PATCH | `/api/games/{id}` | 改状态 / 评分 / 进度 / 时长 / 备注 / 收藏 |
| POST | `/api/games/{id}/refresh-metadata` | 抓取单个条目的商店详情 |
| POST | `/api/imports/steam/wishlist` | 导入 Steam 愿望单（支持 `dry_run` 预览） |
| POST | `/api/imports` | 通用 JSON 导入（离线兜底 / 备份恢复） |
| GET | `/api/imports` | 导入历史 |
| GET | `/api/imports/{id}` | 单次运行详情 |
| POST | `/api/enrich` | 批量补全元数据（限流保护） |
| GET | `/api/export/json` | 全量导出 |

`PATCH` 语义：字段缺省 = 不改动；`rating` / `progress_percent` / `playtime_minutes` 传负数 = 清空；`note` 传空串 = 清空。
状态切到「在玩 / 已通关」会自动补 `started_at` / `finished_at`。

更多示例见 [`docs/api.http`](docs/api.http)（VS Code REST Client 可直接点开跑）。

## 环境变量

| 变量 | 默认值 | 说明 |
|---|---|---|
| `ACGN_BIND` | `127.0.0.1:8787` | 监听地址；要让局域网 / 服务器访问改成 `0.0.0.0:8787` |
| `ACGN_DB` | `./data/acgn.db` | SQLite 文件路径 |
| `ACGN_WEB_DIR` | `./web/dist` | 前端构建产物目录 |
| `ACGN_LOG` | `info,acgn_server=debug…` | tracing 过滤规则 |
| `ACGN_CORS_ORIGINS` | 仅本机 5173 | 跨域白名单；同源部署与 Vite 代理都不需要 CORS |
| `ACGN_STEAM_FIXTURE` | 未设置 | 指向 `fixtures/steam` 进入**离线模式** |
| `STEAM_API_KEY` | 未设置 | 可选；公开接口被拒时回退官方接口 |
| `STEAM_COUNTRY` | `cn` | 商店国家（决定价格币种） |
| `STEAM_LANGUAGE` | `schinese` | 商店语言（决定简介 / 名字语言） |

> 国内网络下 `store.steampowered.com` / `api.steampowered.com` 需要走代理。
> 走代理后如果仍然超时，检查代理的**分流规则**是否把 Steam 规则指向了 DIRECT。

## 目录结构

```
crates/
  acgn-core/    领域模型、DTO、错误、SourceItem 契约
  acgn-store/   SQLite 迁移、仓储、导入合并、运行记录
  acgn-steam/   Steam 取数（公开 JSON / 官方 API / fixture）
  acgn-server/  axum REST API + 静态站点
web/            Vue 3 + Vite + TS 前端
fixtures/steam/ 离线样例数据（wishlist.json / appdetails.json）
scripts/        维护脚本（fiche 生成等）
docs/           接口示例、设计笔记
```

## 开发

```bash
cargo fmt --all                                   # 格式化
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                            # 单测（解析 / 合并 / 迁移）
cd web && pnpm typecheck && pnpm build            # 前端类型检查 + 构建

# 刷新离线样例数据（会真实访问 Steam）
node scripts/gen-steam-fixture.mjs
```

## 路线图

- [x] 领域模型 + SQLite + 导入合并语义
- [x] Steam 愿望单导入 / 元数据补全 / 看板 / 导入历史
- [ ] 游戏价格历史与降价提醒
- [ ] Bangumi 接入：动画 / 漫画 / 小说条目 + 双向进度同步
- [ ] 小黑盒（无公开 API，需调研客户端接口或做手动导入）
- [ ] Tauri 桌面端：复用 `acgn-core` / `acgn-store` / `acgn-steam` + `web`
- [ ] 多端增量同步（基于 `import_runs` 与 `updated_at` 的合并策略）

## 贡献

1. 从 `main` 切分支（`feat/xxx`、`fix/xxx`）
2. 提交信息遵循 `type(scope):subject`（如 `feat(steam):支持个性域名`）
3. 自测通过后提 PR，等待 CI 审查（opencode-review）

## License

AGPL-3.0，见 [`LICENCE`](LICENCE)。
