# 工具执行生命周期简化为 pending/complete 两态

将工具（Tool）执行的事件生命周期从 `pending → doing → complete` 三态回退为 `pending → complete` 两态，并从协议中彻底删除 `ToolDelta` 事件。

## 背景

在 2026-07 之前的设计中，`byte-core::runner::execute_tool_call` 曾对 `grep` / `find_files` 特判，通过 `tool_progress_message` 生成并 emit `ToolDelta`，其他工具直接走 `ToolStarted` → `ToolFinished`。为了统一状态机，之前的 ADR 将生命周期固定为 `pending`（`ToolStarted`）→ `doing`（`ToolDelta`）→ `complete`（`ToolFinished`），并让所有工具都 emit 一条通用 `ToolDelta` 消息。

随着产品演进，我们发现：

- 统一的 `doing` 消息（`正在执行工具 \`{name}\`…`）对用户没有增量价值，前端仅靠 `ToolStarted` 就足以表达"工具已收到"。
- 真正的流式工具输出（如 `run_command` 实时输出）应该通过独立的事件或结果返回机制解决，而不是把 `ToolDelta` 作为中间占位。
- 保留 `ToolDelta` 意味着运行循环和前端都要维护一个仅用于显示通用提示的事件，增加复杂性。

因此，决定把工具生命周期进一步简化，并删除 `ToolDelta`。

## 决策

- 工具执行只保留两个状态：
  - `pending`：`ToolStarted` emitted 时；
  - `complete`：`ToolFinished` emitted 时，携带完整输出与 `is_error`。
- 从 `RuntimeEventKind` 中删除 `ToolDelta` 变体及对应的 `tool_delta` 协议事件。
- 删除所有工具特定的进度文案/进度消息生成逻辑；`RunExecutor` 只负责在 `ToolStarted` 和 `ToolFinished` 之间调用 `tool.invoke`。
- 工具结果（成功或失败）统一由 `ToolFinished` 一次性返回。

## 考虑过的选项

1. **保留三态，继续 emit `ToolDelta`**
   - 否决。通用 `doing` 消息没有增量价值，且运行循环仍保留一个与工具 UX 耦合的占位事件。

2. **保留 `ToolDelta`，但消息由工具自己决定（`Tool::progress`）**
   - 否决。工具进度文案并不足以改善体验，反而让每个工具都需要实现一个文案方法，协议也变得更复杂。

3. **重命名 `ToolDelta` 为 `ToolDoing`**
   - 否决。重命名只解决命名一致性问题，不解决"多余的中间状态"问题，还会引入跨端破坏性变更。

4. **删除 `ToolDelta`，只保留 `ToolStarted` / `ToolFinished`**
   - 采纳。运行循环最小化，协议更简单；前端通过 `ToolStarted` 和尚未收到 `ToolFinished` 的时间窗口表达"执行中"。

## 后果

- **正面**
  - 协议事件数量减少，运行循环职责更清晰：只编排 `ToolStarted` → `invoke` → `ToolFinished`。
  - 前端状态机简化，不再需要 `progressMessage` 字段和 `tool_delta` 分支。
  - 新工具无需考虑进度消息或中间状态，实现成本更低。

- **负面**
  - 慢工具（如 `run_command`）在执行期间没有文本进度反馈，只有状态图标和卡片。
  - 若未来需要真正的流式工具输出，需要设计新的事件类型，不能复用 `ToolDelta`。

- **协议影响**
  - 破坏性变更：`RuntimeEventKind::ToolDelta` 从协议中删除。
  - 前端生成类型和 reducer 需要同步移除 `tool_delta` 处理。

- **受影响文件**
  - `crates/byte-protocol/src/lib.rs`
  - `crates/byte-core/src/runner.rs`
  - `apps/desktop/src/store/types.ts`
  - `apps/desktop/src/store/reducer.ts`
  - `apps/desktop/src/store/reducer.test.ts`
  - `apps/desktop/src/store/selectors.test.ts`
  - `apps/desktop/src/ToolCallCard.tsx`
  - `apps/desktop/src/generated/RuntimeEvent*.ts`
  - `docs/architecture/mvp-architecture.md`
  - `docs/adr/0011-split-tools-skills-registries.md`
