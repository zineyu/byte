# Byte

Byte 是一款本地桌面编程助手，采用 **Tauri v2 + React** 桌面壳与 **Rust 本地 daemon** 的分层架构。桌面壳作为客户端，通过 JSON-RPC over WebSocket 连接由用户手动启动的本地 daemon，并将 daemon 的运行时事件通过 Tauri event 转发到 React 前端。daemon 只监听 `127.0.0.1` 或 `localhost` 地址。

## 技术栈

- **后端 / daemon**：Rust workspace
  - `crates/byte-protocol`：共享的 JSON-RPC 协议、daemon 地址验证、runtime event 类型
  - `crates/byte-daemon`：WebSocket JSON-RPC daemon 入口，由用户手动启动
  - `crates/byte-core`：SessionRunner、运行时服务聚合、LlmContextBuilder
  - `crates/byte-tools`：Tool trait、ToolRegistry 与内置工具实现
  - `crates/byte-skills`：SkillRegistry 与 skill 扫描/激活
  - `crates/byte-models`：模型提供者抽象与 OpenAI-compatible 实现
  - `crates/byte-session`：JSONL 树 session 持久化
- **桌面端**：Tauri v2 + React + Vite + TypeScript（`apps/desktop`）
- **包管理**：Rust 使用 `cargo`，桌面前端使用 `pnpm`

## 目录结构

```
/
├── apps/desktop/          # Tauri v2 + React 桌面应用
│   ├── src/               # React 前端源码
│   └── src-tauri/         # Tauri Rust 源码
├── crates/
│   ├── byte-core/         # 运行时核心（SessionRunner、RuntimeServices）
│   ├── byte-daemon/       # Rust daemon 入口
│   ├── byte-models/       # 模型提供者
│   ├── byte-protocol/     # 共享协议与类型
│   ├── byte-session/      # JSONL session 存储
│   ├── byte-skills/       # skill 注册表与激活
│   └── byte-tools/        # 工具注册表与实现
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

# 启动开发模式（先手动启动 daemon，再启动 Tauri 桌面端）
# 1. 启动 daemon（默认监听 127.0.0.1:8787）
just start-daemon
# 2. 在另一个终端启动桌面端
just start-desktop
# 3. 在设置对话框中输入 daemon 地址，或配置 ~/.config/byte/daemon.toml

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

- `byte-protocol` 同时被 `byte-daemon`、`byte-core`、`byte-tools`、`byte-skills` 和 `apps/desktop/src-tauri` 依赖，所有跨边界类型必须放在这里。
- daemon 负责实现 JSON-RPC 方法、管理运行时状态、执行模型/工具循环；`byte-core` 提供 SessionRunner 与运行时服务聚合。
- `byte-tools` 与 `byte-skills` 实现工具与技能注册表，由 `byte-core` 协调；二者互不依赖。
- 桌面壳只负责连接手动启动的 daemon、维护 WebSocket JSON-RPC 传输、暴露 Tauri command、转发 daemon runtime event，不实现业务逻辑或工具调用。

依赖方向：

```
byte-daemon          → byte-core → byte-tools, byte-skills, byte-models, byte-session
byte-core            → byte-tools, byte-skills, byte-models, byte-session
byte-tools           → byte-protocol
byte-skills          → byte-protocol
byte-models          → byte-protocol
byte-session         → byte-protocol
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
