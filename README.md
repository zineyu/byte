# Byte

Byte 是一款本地桌面编程助手，采用 **Tauri v2 + React** 桌面壳与 **Rust 本地 daemon** 的分层架构。桌面壳通过 LF 分隔的 JSON-RPC over Unix Domain Socket 启动并驱动本地 daemon，同时将 daemon 的运行时事件通过 Tauri event 转发到 React 前端。

## 技术栈

- **后端 / daemon**：Rust workspace
  - `crates/byte-protocol`：共享的 JSON-RPC 协议、JSONL 编解码、daemon 状态类型
  - `crates/byte-daemon`：Unix socket JSONL JSON-RPC daemon 入口
- **桌面端**：Tauri v2 + React + Vite + TypeScript（`apps/desktop`）
- **包管理**：Rust 使用 `cargo`，桌面前端使用 `pnpm`

## 目录结构

```
/
├── apps/desktop/          # Tauri v2 + React 桌面应用
│   ├── src/               # React 前端源码
│   └── src-tauri/         # Tauri Rust 源码
├── crates/
│   ├── byte-daemon/       # Rust daemon 入口
│   └── byte-protocol/     # 共享协议与类型
├── docs/                  # 架构、协议、模型、UI 等文档
│   ├── adr/               # 架构决策记录
│   ├── agents/            # Agent 约定
│   ├── architecture/      # 系统架构
│   ├── desktop/           # 桌面端 UI 约定
│   ├── models/            # 模型提供者配置
│   ├── operations/        # 开发/运行命令
│   └── protocol/          # JSON-RPC 协议术语
├── AGENTS.md              # Agent 协作约定
├── CONTEXT.md             # 领域术语表
├── Cargo.toml             # Rust workspace 配置
└── README.md              # 本文件
```

## 快速开始

详细命令参见 [`docs/operations/commands.md`](docs/operations/commands.md)。

```bash
# 安装桌面前端依赖
cd apps/desktop
pnpm install

# 类型检查 / 构建前端
pnpm run typecheck
pnpm run build

# 启动开发模式（先构建 daemon，再启动 Tauri）
pnpm run tauri:dev

# 审计依赖
pnpm audit --audit-level high
```

Rust 侧验证：

```bash
cargo fmt
cargo clippy
cargo test
```

## 模块边界

- `byte-protocol` 同时被 `byte-daemon` 和 `apps/desktop/src-tauri` 依赖，所有跨边界类型必须放在这里。
- 桌面壳只负责启动 daemon、维护 Unix socket JSONL 传输、暴露 Tauri command、转发 daemon runtime event，不实现业务逻辑或工具调用。
- daemon 负责实现 JSON-RPC 方法、管理运行时状态、执行模型/工具循环。

依赖方向：

```
byte-daemon          → byte-protocol
apps/desktop/src-tauri → byte-protocol
```

禁止反向依赖。

## 文档

- 领域术语与产品语言：[CONTEXT.md](CONTEXT.md)
- Agent 协作约定：[AGENTS.md](AGENTS.md)
- 系统架构总览：[docs/architecture/mvp-architecture.md](docs/architecture/mvp-architecture.md)
- JSON-RPC 协议与术语：[docs/protocol/glossary.md](docs/protocol/glossary.md)
- 模型提供者配置：[docs/models/configuration.md](docs/models/configuration.md)
- 桌面端 UI 约定：[docs/desktop/ui-guidelines.md](docs/desktop/ui-guidelines.md)
- 开发/运行命令：[docs/operations/commands.md](docs/operations/commands.md)

## 安全提醒

- MVP 运行在“无限制本地代理模式”，daemon 会读写文件并执行命令，请在用户信任的本地环境中运行。
- API key 当前以明文方式直接存储在 `ModelProviderConfig` 中，详见 [`docs/adr/0016-remove-secretstore-seam.md`](docs/adr/0016-remove-secretstore-seam.md)。
- 请勿将真实密钥提交到仓库。

## 许可证

[待补充]
