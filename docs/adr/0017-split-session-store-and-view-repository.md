# 拆分 SessionStore 为持久化 seam 与 SessionViewRepository

## 状态

已接受

## 背景

`crates/byte-session` 中的 `SessionStore` 同时承担两类职责：

1. **append-only 持久化**：创建 session、追加 message、删除 session、列出 session 摘要。
2. **视图重建**：读取 JSONL、解析 entry、按 `parent_id` 回溯活跃路径、读取 `AGENTS.md`、组装 `SessionView`。

这导致 `SessionStore` 的公共接口几乎和实现一样宽，JSONL 格式细节也泄漏到视图重建逻辑中。单元测试想要验证活跃路径回溯时，必须构造真实的 `SessionStore` 和文件；想要验证持久化时，又不得不间接通过 `load_session` 观察。

## 决策

- 将 `SessionStore` 缩小为纯粹的持久化 seam：只保留 `new_session`、`append_message`、`delete_session`、`list_sessions`、元数据读取，以及新增的 `read_entries`（读取并解析 JSONL，返回原始 `Vec<SessionEntry>`）。
- 在 `crates/byte-core` 中新增 `SessionViewRepository`，负责从 `read_entries` 返回的原始 entry 重建 `SessionView`、读取 workspace instructions、处理 summary 节点。
- 新增 `SessionViewError`（`MissingHeader`、`BrokenChain`、`Store`），并将 `SessionError` 中的 `MissingHeader` 和 `BrokenChain` 移除。`RunnerError` 增加 `SessionView` 变体，JSON-RPC 错误映射同步更新。
- `RuntimeServices` 在构造时从 `SessionStore` 自动创建 `SessionViewRepository`，供 `SessionManager` 和 `SessionRunner` 共用。

## 后果

- `SessionStore` 不再包含历史树重建或 `AGENTS.md` 读取逻辑；其错误类型也只包含持久化错误。
- `byte-session` 的单元测试可以直接对 `read_entries` 做断言，无需关心视图重建。
- `byte-core` 的单元测试可以通过内存中的 `Vec<SessionEntry>` 直接调用 `SessionViewRepository::build_view`，验证活跃路径回溯和 summary 处理。
- 未来若将底层存储从 JSONL 文件换成 SQLite，只需替换 `SessionStore`；视图重建逻辑不变。
- 变更范围覆盖 `byte-session`、`byte-core`、`byte-daemon` 的 RPC 错误映射，以及新增 ADR。

## 参考

- Issue #43: <https://github.com/zineyu/byte/issues/43>
- ADR-0005: Store sessions as JSONL trees
- ADR-0009: 提取 byte-core 与 SessionRunner 模块
