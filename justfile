# acgn-tools 常用命令（安装 just：https://github.com/casey/just）
set shell := ["bash", "-uc"]

# 默认：列出所有可用命令
default:
    @just --list

# ---------------------------------------------------------------- 开发

# 启动后端（真实网络）
server:
    cargo run -p acgn-server -- serve

# 启动后端（离线 fixture 模式，用仓库里的真实样例数据）
demo:
    ACGN_STEAM_FIXTURE=fixtures/steam cargo run -p acgn-server -- serve

# 启动前端开发服务器（Vite，/api 代理到 8787）
web:
    cd web && pnpm dev

# 前后端一起起（后端在后台）
dev:
    @echo "后端在后台运行，前端前台运行；Ctrl-C 退出前端后记得 kill 后端"
    just demo & cd web && pnpm dev

# ---------------------------------------------------------------- 数据

# 用 fixture 导入一次样例数据
seed:
    ACGN_STEAM_FIXTURE=fixtures/steam cargo run -p acgn-server -- seed

# 打印数据库概览
stats:
    cargo run -p acgn-server -- stats

# 刷新离线样例数据（会真实访问 Steam）
fixture:
    node scripts/gen-steam-fixture.mjs

# 只用现有 wishlist.json 重新生成 library.json（不联网）
fixture-library:
    node scripts/gen-steam-fixture.mjs --library-only

# 只刷新新闻 fixture（联网；appid 来自现有 library/wishlist）
fixture-news:
    node scripts/gen-steam-fixture.mjs --news-only

# 清空本地数据库（谨慎）
reset:
    rm -f data/acgn.db data/acgn.db-shm data/acgn.db-wal
    @echo "已删除本地数据库"

# ---------------------------------------------------------------- 质量

fmt:
    cargo fmt --all

# 与 CI 相同的格式检查（不写文件）
fmt-check:
    cargo fmt --all --check

lint:
    cargo clippy --workspace --all-targets --locked -- -D warnings

test:
    cargo test --workspace --locked

check:
    cargo check --workspace --all-targets --locked

# 前端类型检查 + 构建
web-check:
    cd web && pnpm typecheck

web-build:
    cd web && pnpm build

# 全量自测：与 ci workflow 同一组门（fmt check + clippy + 测试 + 前端类型检查 + 构建）；
# 依次执行、遇错即停（CI 侧会聚合全部失败后再一起报告）
verify: fmt-check lint test web-check web-build

# ---------------------------------------------------------------- 构建

build:
    cargo build --release -p acgn-server

# 单二进制发布：Rust release + 前端产物（由 acgn-server 托管）
dist: build web-build
    @echo "发布产物：target/release/acgn-server + web/dist"
