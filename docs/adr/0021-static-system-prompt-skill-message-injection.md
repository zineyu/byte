# 静态 System Prompt 与 Skill 内容消息流注入

## 背景

issue #11 要求实现本地 Agent Skills 的渐进披露。此前（ADR 0011、ADR 0018）已激活 Skill 的内容由 `LlmContextBuilder` 注入 system prompt 的 "Active skills" 段，且 `SessionRunner` 在每个 model turn 前重建 system prompt，以便 run 中途激活的 Skill 能及时生效。

这一做法有两个问题：

- **Provider 前缀缓存失效**。Skill 激活改变 system prompt 内容后，后续请求的整个前缀都无法命中 prompt cache，多轮 tool-call 循环中代价显著。
- **职责错位**。system prompt 应是稳定的会话前缀；随对话演化的状态（已激活 Skill 内容）不属于这里。

同时，已激活 Skill 内容必须满足两条约束：在上下文中可见，且不被 compaction 摘要掉。

## 决策

- **System prompt 在一次 Run 内只构建一次，且不含已激活 Skill 内容。**
  - 内容 = 固定前导 + 工具定义 + 完整 Skill catalog（name + description，不再区分 active/inactive）+ workspace instruction files。
  - `LlmContextInput` 移除 `active_skills` 字段；`SessionRunner` 删除 per-turn 重建逻辑。

- **已激活 Skill 内容通过消息流（append-only）注入上下文。**
  - 模型触发 `activate_skill`：首次激活的工具结果返回结构化 JSON `{name, description, content}`，作为 tool result 消息追加到历史末尾，下一 model turn 即可见，前缀不变。重复激活只返回简短"已激活"确认，不再重复全文。
  - `build_active_path` 将 `SessionEntry::SkillActivated` 记录转换为**合成 user-role 消息**，位于其在 entry 流中的原始位置；同名多次激活只保留最新快照（与 ADR 0018 的恢复语义一致）。
  - `activate_skill` 的持久化 tool result 消息在重建时被替换为短指针文本（保留 tool call/result 配对，去掉重复全文）。

- **Developer 可用 `/skill:<name>` 显式激活 Skill。**
  - `SessionManager::send_message` 在启动 Run 前解析消息开头的 `/skill:<name>`（仅行首匹配），复用 `SkillRegistry::activate` + `append_skill_activation` + `RunnerPool::record_skill_activation`（与 `ActivateSkillTool` 同一写穿顺序：先持久化后更新内存）。
  - 激活后原始消息文本不变地进入正常 Run；未知 Skill 返回错误且不启动 Run。
  - 不引入独立的"已激活"确认 UI：transcript 中的原始命令文本与随后的 assistant 行为即为反馈（参照 pi 的 expansion-before-send 设计）。

- **Compaction 保护由结构保证。**
  - Compaction range 只引用持久化 `Message` id，合成 Skill 消息不可被 range 覆盖，因此激活内容永不进入摘要范围。
  - 以回归测试固定该行为（`compaction.rs` 与 `active_path.rs` 测试）。

## 考虑过的选项

1. **保留 system prompt 注入，仅在 Skill 变化时重建**
   - 否决。激活发生的瞬间前缀缓存仍然整体失效，且 per-turn 比较/重建逻辑保留了不必要的复杂度。

2. **`/skill:name` 在桌面壳解析**
   - 否决。违反"桌面壳不实现业务逻辑"的模块边界；daemon 侧拦截对所有客户端生效。

3. **`/skill:name` 展开为 pi 式 XML 包裹的用户消息**
   - 否决。byte 已有 `SkillActivated` entry + 消息流注入机制，足以把内容送入上下文；XML 包裹会引入 transcript 渲染剥离的额外问题，且与 ADR 0018 的快照持久化重复。

4. **跳过 `activate_skill` 的 tool result 消息而不是替换为指针**
   - 否决。保留 assistant tool call 但丢掉配对 result 会被 OpenAI 兼容 provider 拒绝。

5. **SessionView 暴露 activated skills 并由桌面端渲染徽章**
   - 暂缓。属于 UI 增强，本决策先用 transcript 自解释方式满足可见性，后续可单独追加。

## 后果

- **正面**
  - system prompt 在 Run 内完全稳定，provider 前缀缓存可命中；Skill 激活（模型触发或 `/skill:` 命令）均不破坏缓存。
  - 已激活 Skill 内容在上下文中只出现一次（首次 tool result 或合成消息），不再双重计费。
  - 压缩保护由 entry 类型结构保证，无需额外标记位。
- **负面**
  - Run 内重复激活同一 Skill 时，模型当轮只看到"已激活"确认；若 Skill 文件恰在会话期间变更，新内容要到下一次 Run 重建 active path 时才对模型可见（`MvpSkillRegistry` 的扫描缓存进一步降低了这种概率，MVP 可接受）。
  - `build_active_path` 需要识别 `activate_skill` 的 tool call/result 配对，逻辑比纯消息过滤复杂，已由单元测试覆盖。
- **对既有决策的影响**
  - ADR 0011 中"`ActivateSkillTool` 结果供 `LlmContextBuilder` 注入 system prompt"的描述由本 ADR 取代（注入路径改为消息流）。
  - ADR 0018 的 `SkillActivated` 持久化与 RunnerPool 语义不变；本 ADR 只改变内容进入 LLM 上下文的路径。
