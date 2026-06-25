# Run 取消事件协议扩展

取消 Run 时，先 flush 尚未发送的 delta，然后 emit `run_cancelled` 事件，并以 `RunStatus::Cancelled` 结束 `run_finished`。为此在 `RunStatus` 枚举中增加 `Cancelled` 变体，在 `RuntimeEventKind` 中增加 `RunCancelled`。

## 背景

协议和代码中一直没有明确的取消语义：`RunStatus` 只有 `Succeeded` 和 `Failed`，也没有 `run_cancelled` 事件。随着 `SessionRunner` 提取和 `cancel_run` JSON-RPC 命令的实现，需要定义取消在事件层如何表示。

## 决策

- `RunStatus` 增加 `Cancelled` 变体。
- `RuntimeEventKind` 增加 `RunCancelled { run_id: String }`。
- `SessionRunner::cancel_run` 的行为：
  1. 取消 provider stream 的后续处理；
  2. 把当前 `delta_buffer` 中未 flush 的内容作为 `message_delta` 发出；
  3. emit `run_cancelled`；
  4. emit `run_finished(status = Cancelled)`。
- UI 层可以把 `run_cancelled` 视为显式的生命周期事件，用于展示「已取消」状态。

## 考虑过的选项

1. **取消直接 emit `run_finished(Failed, Some("cancelled"))`**：改动最小，但把取消与失败混为一谈，未来 UI 难以区分。
2. **只新增 `run_cancelled` 事件，不修改 `RunStatus`**：事件层可以识别，但 `RunStatus` 枚举不完整，`run_finished` 仍需要某种状态表示。
3. **取消时丢弃剩余 delta**：实现最简单，但用户会失去已经生成的部分 assistant content，体验较差。

最终选择同时扩展 `RunStatus` 和 `RuntimeEventKind`，并在取消前 flush 剩余 delta，以提供完整、明确的取消语义。

## 后果

- 协议出现破坏性变更：`RunStatus` 新增变体。当前 MVP 没有稳定版本承诺，桌面端与 daemon 同步升级即可。
- UI reducer 需要处理 `run_cancelled` 和 `RunStatus::Cancelled`。
- 取消路径可以被单元测试明确断言：事件序列包含 `message_delta`（如有剩余）、`run_cancelled`、`run_finished(Cancelled)`。
