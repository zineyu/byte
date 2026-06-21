## Agent skills

### Issue tracker

Issues and PRDs are tracked in GitHub Issues for `zineyu/byte`. See `docs/agents/issue-tracker.md`.

### Triage labels

Triage uses the default five-label vocabulary. See `docs/agents/triage-labels.md`.

### Domain docs

Domain documentation uses a multi-context layout. See `docs/agents/domain.md`.

## 项目概述

Byte Agent 是一个本地桌面编程助手，采用 **Tauri v2 + React** 桌面壳与 **Rust 本地 daemon** 分层的架构。当前骨架通过 LF 分隔的 JSON-RPC over Unix Domain Socket 启动并驱动本地 daemon，并通过 Tauri event 将 daemon runtime event 转发到 React。

## 技术栈

- **后端 / daemon**：Rust workspace（`crates/*`）
  - `byte-protocol`：共享的 JSON-RPC 协议、JSONL 编解码、daemon 状态类型
  - `byte-daemon`：Unix socket JSONL JSON-RPC daemon 入口
- **桌面端**：Tauri v2 + React + Vite + TypeScript（`apps/desktop`）
- **包管理**：
  - Rust：`cargo`
  - 桌面前端：`pnpm`

## 目录结构

```
/
├── Cargo.toml                       # Rust workspace 根配置
├── Cargo.lock
├── crates/
│   ├── byte-protocol/               # 协议 crate
│   └── byte-daemon/                 # daemon 入口
├── apps/
│   └── desktop/                     # Tauri v2 + React 桌面应用
│       ├── package.json
│       ├── pnpm-lock.yaml
│       ├── index.html
│       ├── vite.config.ts
│       ├── tsconfig.json
│       ├── src/                     # React 前端源码
│       └── src-tauri/               # Tauri Rust 源码
├── docs/
│   ├── adr/                         # 系统级架构决策记录
│   ├── agents/                      # agent 约定文档
│   └── architecture/                # 架构文档
├── CONTEXT.md                       # 领域术语表
└── AGENTS.md                        # 本文件
```

## 常用命令

### Rust workspace

```bash
# 构建
cargo build

# 运行 daemon Unix socket smoke test
cargo test -p byte-daemon --test unix_socket_json_rpc

# 格式、检查、测试
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

### 桌面应用

```bash
cd apps/desktop

# 安装依赖
pnpm install

# 类型检查 / 构建前端
pnpm run typecheck
pnpm run build

# 启动开发模式（先构建 daemon，再启动 Tauri）
pnpm run tauri:dev

# 审计
pnpm audit --audit-level high
```

## 模块边界与复用约定

- **协议共享**：`byte-protocol` 同时被 `byte-daemon` 和 Tauri (`apps/desktop/src-tauri`) 依赖，所有跨边界类型必须放在这里，禁止在前端或 daemon 里重新定义 JSON-RPC 结构。
- **桌面壳职责**：只负责启动 daemon、维护 Unix socket JSONL 传输、暴露 `get_daemon_state` 等 Tauri command，并把 daemon runtime event 转发为 Tauri event。不实现业务逻辑或工具调用。
- **daemon 职责**：实现 JSON-RPC 方法、管理运行时状态、执行模型/工具循环。未来通过 `byte-protocol` 扩展命令面。
- **依赖方向**：
  - `byte-daemon` → `byte-protocol`
  - `apps/desktop/src-tauri` → `byte-protocol`
  - 禁止反向依赖

## 测试策略

- Rust：JSONL framing 与 request/response correlation 必须有单元/集成测试覆盖。
- 前端：typecheck 通过；后续引入组件测试时再补充。
- 端到端：Tauri 与 daemon 的集成在 CI 中通过编译检查保证；完整 GUI 交互测试暂不强制。

## 提交与版本控制

- 使用 `jj` 进行版本控制，禁止直接执行 `git`。
- commit message 使用中文。
- 完成修改并通过验证后，先说明变更、验证结果和建议提交信息；只有用户确认后，才执行 `jj desc` 或 `jj git push`。
- 多文件、跨模块、接口、数据库、权限、状态流转、异步或并发改动前，先给出方案、范围、风险和验收标准，等待用户确认。

## CI 质量门

仓库已配置 `.github/workflows/ci.yml`，执行：

- 仓库文档完整性检查
- Markdown 基础检查
- Rust：`cargo fmt`、`cargo clippy`、`cargo test`
- 桌面前端：依赖安装、`lint/typecheck/test/build`（存在时）、`audit`

当前 `apps/desktop/package.json` 未配置 `lint` 和 `test` script，CI 会跳过对应步骤；补充时请保持锁文件同步更新。

## 安全与风险提醒

- MVP 运行在“无限制本地代理模式”，daemon 会读写文件并执行命令，仅在用户信任的本地环境中运行。
- API key 当前以明文方式存储在可替换的 `SecretStore` 中（见 `docs/adr/0007-store-mvp-secrets-in-plaintext-config.md`）。
- 不要提交真实密钥到仓库。
