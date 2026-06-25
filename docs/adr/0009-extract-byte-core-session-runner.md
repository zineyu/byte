# 提取 byte-core 与 SessionRunner 模块

将 Core Coding Loop 从 `byte-daemon/src/main.rs` 提取到独立的 `crates/byte-core`，由 `SessionRunner` 负责单个 Session 的 Run 生命周期，`SessionManager` 负责跨 Session 的 runner 管理与 session CRUD。`byte-daemon` 退化为 Unix socket 传输适配器与 JSON-RPC dispatch 层。

## 背景

`byte-daemon/src/main.rs` 直接实现了 RPC 派发、Run 生命周期、事件流、session append、active-runs 清理等全部运行时职责，接口几乎等于实现，成为 shallow monolith。测试一次 Run 必须启动 Unix socket 并构造真实或半真实依赖。

## 决策

- 新建 `crates/byte-core`，包含 `SessionRunner`、`SessionManager`、`RunExecutor`、`RuntimeEventBus` trait 与 `RunnerError`。
- `SessionRunner` 是 per-session 长期实例，维护本会话「一次只能有一个活跃 Run」的约束，提供 `send_message`、`cancel_run`、`is_running`。
- `SessionManager` 持有 `HashMap<session_id, Arc<SessionRunner>>`，封装 session CRUD、`send_message`、`cancel_run`。
- `RuntimeEventBus` 通过 trait 抽象，daemon 使用基于 `broadcast::Sender<RuntimeEvent>` 的实现，测试使用 recording 实现。
- `ModelProvider` 与 `SessionStore` 在 daemon 启动时构造并注入 `SessionManager`；`byte-core` 只依赖它们的接口或 seam。
- `byte-daemon` 新增 `rpc.rs` 集中所有 JSON-RPC handler；`main.rs` 只负责 socket accept、JSONL 编解码、事件转发和依赖注入。

## 考虑过的选项

1. **把 SessionRunner 保留在 `byte-daemon` 内**：改动最小，但核心循环仍与 daemon 绑定，未来拆出 SDK/CLI 时需要再搬一次。
2. **每次 Run 新建一个短期 SessionRunner**：粒度更小，但会把 one-active-run-per-session 和 cancel 协调逻辑推到另一层，与架构文档中「Owns the conversation loop for one session」的表述不一致。
3. **SessionManager 只封装 send_message/cancel_run，CRUD 留在 RPC handler**：实现更简单，但 session 生命周期逻辑会分散在两个层级。

最终选择新建 `byte-core` 并由 `SessionManager` 统一封装，是因为它能保持 daemon 层最薄，同时让核心循环可独立测试和复用。

## 后果

- `byte-daemon/src/main.rs` 的行数和职责大幅下降。
- 单元测试可以使用 `EchoProvider` + 临时 `SessionStore` + mock event bus 完成完整 Run，无需 socket。
- 未来 CLI、SDK 或测试 harness 可以直接依赖 `byte-core`，不必经过 daemon。
