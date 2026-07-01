# 工具执行生命周期简化为 pending/doing/complete 三态

将工具（Tool）执行的事件生命周期明确为 `pending` → `doing` → `complete` 三个状态，移除 `grep` / `find_files` 等工具的专属进度文案，`ToolDelta` 统一作为每个工具调用的 `doing` 状态事件。

## 背景

在此之前，`byte-core::runner::execute_tool_call` 只对 `grep` 和 `find_files` 通过 `tool_progress_message` 生成并 emit `ToolDelta`，其他工具直接走 `ToolStarted` → `ToolFinished`。这导致：

- 工具生命周期的事件语义不一致：有的工具有“进行中”状态，有的没有；
- UI 无法对所有工具采用统一的状态机渲染；
- `tool_progress_message` 把具体工具参数（pattern、path）硬编码进 runner，污染核心循环。

随着 `RunExecutor` 被重写为更薄的编排函数，工具状态机的简化时机成熟。

## 决策

- 工具执行固定为三个状态：
  - `pending`：`ToolStarted`  emitted 时；
  - `doing`：`ToolDelta`  emitted 时，消息固定为 `正在执行工具 \`{name}\`…`；
  - `complete`：`ToolFinished` emitted 时，携带完整输出与 `is_error`。
- 每个工具调用都必须 emit `ToolDelta`，不再按工具类型区分。
- 删除 `tool_progress_message` 函数，runner 不再关心具体工具的搜索参数。
- 保持 `ToolDelta` 事件协议不变（仍含 `tool_call_id` 与 `message`），只改变其语义：从“可选进度文案”变为“确定性的 doing 状态”。

## 考虑过的选项

1. **保留 `tool_progress_message`，继续只对部分工具 emit `ToolDelta`**
   - 否决。生命周期不一致，且 runner 持续依赖工具细节。

2. **删除 `ToolDelta`，只保留 `ToolStarted` / `ToolFinished`**
   - 否决。只剩两个状态，无法表达“工具已收到、正在执行”的中间状态，UI 在慢工具（如 `run_command`）上会出现长时间无反馈。

3. **重命名或新增 `ToolDoing` 事件，废弃 `ToolDelta`**
   - 否决。MVP 没有稳定协议承诺，但当前桌面端已经消费 `ToolDelta`；重命名会引入无必要的破坏性变更。保留 `ToolDelta` 并改变其语义是更保守的改法。

## 后果

- **正面**
  - 工具生命周期对所有工具一致，UI 可用统一三态模型渲染。
  - `RunExecutor` 不再包含工具特定的文案逻辑，职责边界更清晰。
  - 后续若需要真正的流式工具输出，可以在 `doing` 状态下扩展 `ToolDelta`，而不破坏三态框架。

- **负面**
  - `ToolDelta` 的 message 失去对 `grep` / `find_files` 的具体参数描述，桌面端若依赖旧文案需要调整。
  - 暂时不支持在 `doing` 阶段展示差异化进度（如“正在搜索第 3 个文件”），统一为通用提示。

- **协议影响**
  - 无破坏性字段变更；`RuntimeEventKind::ToolDelta` 的 `message` 内容由具体文案变为通用文案。
