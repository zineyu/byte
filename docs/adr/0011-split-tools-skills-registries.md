# 拆分 byte-tools、byte-skills 与 registry seam

将工具（Tool）、技能（Skill）与各自的注册表从 `byte-daemon` 与 `byte-core::prompt` 中拆分为独立 crate，并明确 `ToolPolicy`、`activate_skill`、跨边界类型与依赖方向的归属，使核心循环可独立测试、registry 可替换。

## 背景

issue #19 要求补齐 `byte-tools`、`byte-skills` 与 `byte-core` crate 及 registry seam。在此之前：

- `byte-daemon` 吸收了本应由独立 crate 承担的工具/技能管理职责。
- `byte-core::prompt` 硬编码了 `ToolDefinition` 与 skill 占位，无法对接真实的 `ToolRegistry` / `SkillRegistry`。
- `SessionRunner` 只能流式输出文本，无法执行模型请求的工具调用，也无法处理 `activate_skill`。
- `byte-protocol` 缺少 `ToolCall`、`ToolResult` 等跨边界类型；运行时无法产生 `tool_started`、`tool_finished` 等事件。

因此需要将工具定义、注册、执行与技能扫描、激活拆分为可独立测试的模块，并在 `byte-protocol` 统一跨边界类型。

## 决策

- **新建 `crates/byte-tools`**
  - 存放 `Tool` trait、`ToolPolicy` seam、`ToolRegistry` trait、`MvpToolRegistry` 与 7 个内置工具实现：`read_file`、`write_file`、`apply_patch`、`run_command`、`list_directory`、`grep`、`find_files`。
  - `ToolPolicy` 定义在 `byte-tools` 中，MVP 提供 `AllowAllPolicy`。

- **新建 `crates/byte-skills`**
  - 存放 `SkillRegistry` trait 与 `MvpSkillRegistry`。
  - 负责 user / workspace skill 目录扫描、同名覆盖与 frontmatter 解析。

- **跨边界类型统一放在 `byte-protocol`**
  - `ToolDefinition`、`ToolCall`、`ToolResult`、`SkillEntry`、`SkillDefinition`、`ActivatedSkill`、`SessionContext` 等类型定义在 `byte-protocol`，供 `byte-tools`、`byte-skills`、`byte-core`、`byte-daemon` 与桌面端共享。
  - `ToolDefinition.parameters` 使用 JSON Schema 形式的 `serde_json::Value`，直接供给 OpenAI tools API。

- **`ToolPolicy` 放在 `byte-tools`**
  - `ToolPolicy::check(call: &ToolCall, ctx: &SessionContext) -> Result<(), ToolError>` 定义在 `byte-tools`，可被具体工具实现与 `MvpToolRegistry` 引用。
  - `MvpToolRegistry` 在构造时注入 per-tool policy，并在 `invoke` 内部先 check 再执行。

- **`activate_skill` 在 `byte-core` 实现并动态注册**
  - `activate_skill` 作为特殊工具，由 `byte-core` 的 `ActivateSkillTool` 实现。
  - 每个 `SessionRunner` 通过 `SessionToolRegistry` 将 `ActivateSkillTool` 动态包装到基础 `MvpToolRegistry` 之上，注入本会话的 `active_skills` 状态，而不修改共享的基础 registry。
  - `ActivateSkillTool` 持有 `Arc<dyn SkillRegistry>`，调用 `SkillRegistry::activate` 后将结果追加到 `SessionRunner` 的内存 `active_skills` 列表，供后续 Run 的 `LlmContextBuilder` 注入上下文。

- **`RuntimeServices` 在 `byte-core` 聚合依赖**
  - `byte-core` 引入 `RuntimeServices`，聚合 `provider`、`store`、`event_bus`、`tool_registry`、`skill_registry`。
  - `SessionManager` 与 `SessionRunner` 通过 `RuntimeServices` 统一接收依赖，避免构造参数膨胀。

- **依赖方向**
  - `byte-daemon` → `byte-core`
  - `byte-core` → `byte-tools`、`byte-skills`、`byte-models`、`byte-session`
  - `byte-tools`、`byte-skills`、`byte-models`、`byte-session`、`byte-daemon`、`apps/desktop/src-tauri` 均 → `byte-protocol`
  - `byte-tools` 与 `byte-skills` 互不依赖；`byte-core` 是唯一的协调层。

- **Skill 扫描规则**
  - 扫描目录顺序：
    1. `~/.agents/skills/`
    2. `~/.byte/skills/`
    3. `<workspace>/.agents/skills/`
    4. `<workspace>/.byte/skills/`
  - 后扫描的覆盖先扫描的，因此 workspace skill 覆盖 user skill。
  - skill 名称取自 `skill.md` frontmatter 的 `name` 字段；描述取自 frontmatter 的 `description` 字段，否则取第一个 Markdown 标题。

## 考虑过的选项

1. **把 `ToolPolicy` 放在 `byte-core`**
   - 否决。`ToolPolicy` 需要被 `byte-tools` 中的 `MvpToolRegistry` 与具体工具实现引用；放在 `byte-core` 会引入 `byte-tools` → `byte-core` 的反向依赖，破坏 crate 边界。

2. **把 `ToolRegistry` / `SkillRegistry` 的默认实现放在 `byte-core`**
   - 否决。这会让 `byte-core` 膨胀并承担本应由独立 crate 负责的具体实现，降低可测试性；也会让未来替换 registry 实现时不得不改动 `byte-core`。

3. **把 `activate_skill` 放在 `byte-skills`**
   - 否决。`activate_skill` 是一个 tool，需要实现 `byte-tools` 的 `Tool` trait；若放在 `byte-skills`，会导致 `byte-skills` 依赖 `byte-tools`，而 `byte-core` 又需要同时使用两者，存在循环依赖风险，并模糊「工具 seam」与「技能 seam」的边界。

4. **把 `activate_skill` 放在 `byte-tools`**
   - 否决。`activate_skill` 需要访问 `SkillRegistry` 并修改 `SessionRunner` 的 per-session `active_skills` 状态；放在 `byte-tools` 会迫使 `byte-tools` 依赖 `byte-skills` 或 `byte-core`，破坏依赖方向。

5. **让 `SessionRunner` 直接持有所有依赖的单独字段**
   - 否决。随着 registry、provider、store、event bus 增多，构造参数会迅速膨胀；`RuntimeServices` 作为聚合容器更清晰，也便于测试替换。

## 后果

- **正面**
  - `byte-tools` 与 `byte-skills` 拥有清晰的 crate 边界，可独立编译和测试。
  - 具体工具可在临时目录中独立测试，无需启动 daemon。
  - `ToolPolicy` 与 `SkillRegistry` 都是 trait，后续替换实现无需改动 `byte-core`。
  - `byte-protocol` 成为唯一跨边界类型定义位置，避免前后端类型漂移。
  - `byte-daemon` 退化为依赖注入与 JSON-RPC 传输层。

- **负面**
  - Workspace 中 crate 数量增加，初次构建时间略有上升。
  - MVP 中 `MvpToolRegistry` 与 `MvpSkillRegistry` 仍是单例 / 进程内实现，未解决多进程共享问题。
  - `activate_skill` 的激活状态保存在 `SessionRunner` 内存中，daemon 重启后清空，未持久化到 session 文件。
    - **注**：此状态已被 `docs/adr/0018-persist-activated-skills-and-runner-pool.md` 修正。Skill 激活状态现在持久化为 `SessionEntry::SkillActivated`，`RunnerPool` 独立管理 runner 生命周期，runner 被回收后重建时可以从 session 文件恢复已激活 skills。
  - `run_command` 等工具当前返回完整输出，未实现流式 `command_output`，后续扩展接口时需要谨慎保持向后兼容。
