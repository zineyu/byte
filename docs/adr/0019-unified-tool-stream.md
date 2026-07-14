# 统一工具调用接口为 stream

## 状态

Accepted

## 日期

2026-07-14

## 背景

issue #45 要求统一 byte 中 `Tool` 的调用接口。此前工具接口存在两套路径：

- `Tool::invoke(...)` 返回 `Result<String, ToolError>`，用于非流式工具；
- `Tool::invoke_with_sink(...)` 接收一个 `ToolEventSink`，用于流式工具（如 `run_command`）emit 增量输出。

`run_command` 是唯一真正实现流式输出的工具，其他工具仅使用 `invoke` 并返回最终结果。两套接口带来以下问题：

1. **接口不一致**：调用者必须根据工具是否流式选择不同入口，注册表需要同时传递 sink。
2. **测试复杂**：流式测试需要实现 `ToolEventSink`（如 `RecordingSink`），非流式测试则直接等待字符串结果。
3. **抽象泄漏**：`ToolEventSink` 和 `NoopEventSink` 暴露了本应由工具内部控制的输出机制。
4. **未来扩展困难**：若后续新增其他流式工具（如长时间运行的进程、文件流式读取），需要重新定义或复制 sink 模式。

## 决策

**统一所有 `Tool::invoke` 返回 `Result<ToolOutputStream, ToolError>`。**

- `ToolOutputStream` 是一个 pinned、Send 的 `Stream<Item = Result<ToolStreamEvent, ToolError>>`。
- `ToolStreamEvent` 包含两类事件：
  - `Chunk { chunk: String }`：增量输出，仅流式工具（如 `run_command`）会产生。
  - `Done { result: ToolOutputResult }`：工具执行结束，携带最终结果。
- 非流式工具通过 `futures::stream::once` 产生单一 `Done` 事件。
- `run_command` 使用 `tokio::spawn` + `futures::channel::mpsc` 在后台读取 stdout/stderr，并 emit chunk 事件，最后 emit `Done`。
- 删除 `ToolEventSink`、`NoopEventSink` 以及 `invoke_with_sink`。
- `ToolRegistry::invoke` 不再接收 sink，返回 `ToolOutputStream`。
- `byte-core` 的 `RunExecutor` 消费 stream：emit `ToolStarted`，转发每个 `Chunk` 为 `ToolOutputDelta`，`Done` 后 emit `ToolFinished`；若 stream 未产生 `Done` 就结束，则按错误处理。
- `ToolError` 表示“工具没能跑起来”（参数缺失、未知工具、policy 拒绝）；`ToolOutputResult` 表示“工具跑完后的结果”（包括非零退出码、超时等）。

## 考虑过的选项

### 1. 保持 `invoke` + `invoke_with_sink` 两套接口

- 优点：改动最小，`run_command` 保持现有实现。
- 缺点：抽象不一致，调用者和测试需要区分流式/非流式；issue #45 明确要求统一接口。
- 否决。

### 2. 拆分为 `Tool` 和 `StreamingTool` 两个 trait

- 优点：类型层面区分流式与非流式，非流式工具接口保持简单。
- 缺点：需要为 `run_command` 单独定义 trait，注册表和 runner 需要同时处理两种 trait 对象；增加了抽象复杂度，且 issue #45 最终决策为不拆分。
- 否决。

### 3. 统一为返回 `Stream` 的单一 `invoke`（本决策）

- 优点：接口一致，调用者统一消费 stream；非流式工具用单事件 stream 即可；删除了 sink 抽象；未来扩展流式工具无需改动接口。
- 缺点：非流式工具的返回类型从 `String` 变为 `Stream`，调用方代码稍长；需要引入 `futures` 依赖。
- 采纳。

## 后果

### 正面

- 工具接口单一化，调用者、注册表、runner 不再区分流式/非流式入口。
- 删除 `ToolEventSink`、`NoopEventSink` 等测试辅助类型，降低测试复杂度。
- `run_command` 的流式输出与 runner 的事件转发路径更清晰，便于后续增加流式工具。
- `ToolError` 与 `ToolOutputResult` 的语义边界明确：前者代表调用失败，后者代表执行结果（含错误退出码）。

### 负面

- 非流式工具需要包装成单事件 stream，调用方必须消费 stream 才能拿到结果。
- `byte-tools` 新增 `futures` 依赖。
- 现有测试中对 `Tool::invoke` 返回字符串的调用点需要改为消费 stream。

## 参考

- issue #45: https://github.com/zineyu/byte/issues/45
- 实现涉及文件：`crates/byte-tools/src/lib.rs`、`crates/byte-tools/src/registry.rs`、`crates/byte-tools/src/run_command.rs`、所有同步工具实现、`crates/byte-core/src/runner.rs`、`crates/byte-core/src/activate_skill.rs`。
