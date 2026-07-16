## Agent skills

### Issue tracker

Issues and PRDs are tracked in GitHub Issues for `zineyu/byte`. See `docs/agents/issue-tracker.md`.

### Triage labels

Triage uses the default five-label vocabulary. See `docs/agents/triage-labels.md`.

### Domain docs

Domain documentation uses a multi-context layout. See `docs/agents/domain.md`.

## Documentation index

Use this index to find the right doc before making changes.

| When you are working on… | Read first |
| ------------------------ | ---------- |
| Desktop UI 设计系统、设计 token 与视觉原则 | `DESIGN.md` |
| JSON-RPC protocol, runtime events, message roles, or the `send_message` flow | `docs/protocol/glossary.md` |
| Model provider config (`~/.config/byte/config.toml`) or provider abstraction | `docs/models/configuration.md` |
| Desktop UI layout, icons, colors, or input interaction | `docs/desktop/ui-guidelines.md` |
| Build/test commands or local development workflow | `docs/operations/commands.md` |
| System-wide architecture decisions | `docs/adr/` |
| Agent conventions (skills, triage, issue tracker) | `docs/agents/` |
| Overall system architecture and component boundaries | `docs/architecture/mvp-architecture.md` |
| Domain terminology and product language | `CONTEXT.md` |
## 项目概述

Byte Agent 是一个本地桌面编程助手，采用 **Tauri v2 + React** 桌面壳与 **Rust 本地 daemon** 分层的架构。当前骨架通过 JSON-RPC over WebSocket 连接由用户手动启动的本地 daemon，并通过 Tauri event 将 daemon runtime event 转发到 React。daemon 只监听 `127.0.0.1` 或 `localhost` 地址。

## 技术栈

- **后端 / daemon**：Rust workspace（`crates/*`）
  - `byte-protocol`：共享的 JSON-RPC 协议、daemon 地址验证、runtime event 类型
  - `byte-daemon`：WebSocket JSON-RPC daemon 入口，由用户手动启动
  - `byte-core`：SessionRunner、运行时服务聚合、LlmContextBuilder
  - `byte-tools`：Tool trait、ToolRegistry 与内置工具实现
  - `byte-skills`：SkillRegistry 与 skill 扫描/激活
  - `byte-models`：模型提供者抽象与 OpenAI-compatible 实现
  - `byte-session`：JSONL 树 session 持久化
- **桌面端**：Tauri v2 + React + Vite + TypeScript（`apps/desktop`）
- **包管理 / 任务运行**：
  - Rust：`cargo`
  - 桌面前端：`pnpm`
  - 本地验证与自动化：`just`

## 目录结构

```
/
├── docs/
│   ├── adr/                         # 系统级架构决策记录
│   ├── agents/                      # agent 约定文档
│   ├── architecture/                # 系统架构文档
│   ├── desktop/                     # 桌面端 UI 约定
│   ├── models/                      # 模型提供者配置
│   ├── operations/                  # 开发/运行命令
│   └── protocol/                    # JSON-RPC 协议术语
├── DESIGN.md                       # 桌面端设计系统规范
├── CONTEXT.md                       # 领域术语表
└── AGENTS.md                        # 本文件
│       ├── package.json
│       ├── pnpm-lock.yaml
│       ├── index.html
│       ├── vite.config.ts
│       ├── tsconfig.json
│       ├── src/                     # React 前端源码
│       └── src-tauri/               # Tauri Rust 源码
├── docs/
## 常用命令

常用命令已整理到 `docs/operations/commands.md`。本地验证统一使用根目录 `Justfile`：

```bash
# 完整验证，覆盖 CI 中的仓库检查、Rust、桌面前端和审计质量门
just verify

# 子验证；Rust/desktop 子验证内置对应 fmt-check
just verify repo
just verify design-md
just verify workflow
just verify rust
just verify desktop
just verify audit

# 格式化 / 格式检查：Rust 使用 cargo fmt，前端 TS/CSS/HTML 使用 Prettier
just fmt
just fmt-check

# 启动开发模式（先手动启动 daemon，再启动 Tauri 桌面端）
# 1. 启动 daemon（默认监听 127.0.0.1:8787）
just start-daemon
# 2. 在另一个终端启动桌面端
just start-desktop
# 3. 在设置对话框中输入 daemon 地址，或配置 ~/.config/byte/daemon.toml
```

## 模块边界与复用约定

- **协议共享**：`byte-protocol` 同时被 `byte-daemon`、`byte-core`、`byte-tools`、`byte-skills` 和 `apps/desktop/src-tauri` 依赖，所有跨边界类型必须放在这里，禁止在前端或 daemon 里重新定义 JSON-RPC 结构。
- **daemon 职责**：实现 JSON-RPC 方法、管理运行时状态、执行模型/工具循环；`byte-core` 提供 SessionRunner 与运行时服务聚合。
- **工具/技能注册表**：`byte-tools` 与 `byte-skills` 实现工具与技能注册表，由 `byte-core` 协调；二者互不依赖。
- **桌面壳职责**：只负责连接手动启动的 daemon、维护 WebSocket JSON-RPC 传输、暴露 `get_daemon_state` 等 Tauri command，并把 daemon runtime event 转发为 Tauri event。不实现业务逻辑或工具调用。
- **依赖方向**：
  - `byte-daemon` → `byte-core` → `byte-tools`, `byte-skills`, `byte-models`, `byte-session`
  - `byte-tools`, `byte-skills`, `byte-models`, `byte-session`, `byte-daemon`, `apps/desktop/src-tauri` → `byte-protocol`
  - 禁止反向依赖

## 设计系统约定

- **DESIGN.md 是前端视觉的唯一来源**：`apps/desktop` 的所有 UI 颜色、字体、圆角、间距和组件风格必须以 `DESIGN.md` 中的设计 token 和 prose 为准，禁止在组件或样式表中引入新的主色、新的字体堆栈或新的圆角体系。
- **UI 改动同步回 DESIGN.md**：修改前端样式、布局或新增组件时，必须同步更新 `DESIGN.md` 中的 token、组件定义和 prose 描述，确保设计文档始终反映真实界面。
- **DESIGN.md 校验是质量门的一部分**：提交前需保证 `npx @google/design.md lint DESIGN.md` 没有 errors；warnings 必须是真实设计意图且已在 prose 中说明，禁止为通过校验而伪造 token 或颜色。
- **工具链**：使用 `just verify` 或 `just verify repo` 自动运行 DESIGN.md 校验；本地也可直接运行 `npx @google/design.md lint DESIGN.md`。

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

仓库已配置 `.github/workflows/ci.yml`，本地对应入口为 `just verify`，覆盖：

- 仓库文档完整性检查
- Markdown 基础检查
- Workflow YAML 解析
- Rust：`cargo fmt`、`cargo clippy`、`cargo test`
- 桌面前端：依赖安装、`lint/typecheck/test/build`（存在时）、`audit`

当前 `apps/desktop/package.json` 未配置 `lint` script，CI 与 `just desktop` 会跳过对应步骤；补充时请保持锁文件同步更新。

## 安全与风险提醒

- MVP 运行在“无限制本地代理模式”，daemon 会读写文件并执行命令，仅在用户信任的本地环境中运行。
- API key 当前以明文方式直接存储在 `ModelProviderConfig` 中（见 `docs/adr/0016-remove-secretstore-seam.md`）。
- 不要提交真实密钥到仓库。
