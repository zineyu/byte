# 持久化已激活 Skill 并通过 RunnerPool 管理 SessionRunner 生命周期

## 背景

issue #44 要求为 `SessionRunner` 引入 `RunnerPool` 生命周期管理。此前 `SessionManager` 直接持有一个 `HashMap<session_id, Arc<SessionRunner>>`，runner 除非调用 `delete_session`，否则永远不会从 map 中移除。此外，`activate_skill` 的激活状态保存在 `SessionRunner` 内存中，这意味着 runner 被回收后 Skill 状态会丢失，daemon 重启后也无法恢复。

## 决策

- **`SessionRunner` 是短生命周期的执行对象，不是 Session 状态的容器。**
  - runner 只负责一次或多次 Run 的执行循环，不承载需要跨 runner 保留的 Session 状态。
  - 已激活 Skill 属于 Session 状态，而不是 runner 状态。

- **激活 Skill 状态持久化到 Session JSONL。**
  - 新增 `SessionEntry::SkillActivated { name, content }`，保存激活时的完整内容快照。
  - 不重新从磁盘加载最新 Skill 内容，避免 Skill 文件后续变更导致历史 Session 的行为被悄悄修改。
  - 对同一 Skill 的多次激活，session 文件中会记录多条快照；重建 runner 时取每个 Skill 名称的最新快照。

- **`ActivateSkillTool` 在修改内存 `active_skills` 之前先把快照写入 session store。**
  - 写入失败作为 tool 错误返回，防止内存与持久化状态不一致。
  - 当 `SessionContext.session_id` 缺失时跳过持久化（仅在测试中）。

- **新增 `RunnerPool` 模块管理 runner 缓存。**
  - `RunnerPool` 位于 `byte-core`，对 `SessionManager` 暴露窄接口：
    - `get_or_create(session_id) -> Arc<SessionRunner>`
    - `close(session_id) -> CloseResult`
  - `RunnerPool` 独立持有每个 session 的 `active_skills`（通过 `Arc<Mutex<...>>` 共享给 runner），runner 回收后状态仍留在 pool 中。
  - 当前 MVP 只支持显式 `close`（由 `delete_session` 调用），后台 idle 回收留作后续。

- **`SessionManager` 不再直接持有 `HashMap<session_id, Arc<SessionRunner>>`。**
  - 所有 runner 访问都通过 `RunnerPool`。

## 考虑过的选项

1. **只迁移 HashMap，不持久化 Skill 状态**
   - 否决。这样 runner 回收后已激活 Skill 会丢失，改变现有语义，且 daemon 重启后状态不可恢复。

2. **持久化 Skill 名称而非内容快照**
   - 否决。重建时重新从 `SkillRegistry` 加载会受 Skill 文件后续变更影响，且 Skill 被删除后无法恢复。

3. **把 Skill 激活作为普通 `Message` 记录**
   - 否决。Skill 激活不是对话消息，混入 `Message` 会污染 `SessionView.messages`，并在聊天 UI 中显示无意义节点。

4. **让 `RunnerPool` 在 runner 回收时把 `active_skills` 从 runner 同步回 pool**
   - 否决。直接让 pool 持有 `active_skills` 的共享 Arc 更简洁，消除了回收同步步骤和潜在竞争。

5. **在 `RunnerPool` 中引入 LRU / idle timeout 后台回收**
   - 否决。MVP session 数量少，显式 `close` 已足够；后台回收会增加测试复杂度和配置面。

## 后果

- **正面**
  - `SessionManager` 不再直接操作 runner 缓存，runner 生命周期策略被封装。
  - runner 可以安全回收或重建，Skill 激活状态不会丢失。
  - daemon 重启后可以从 session 文件恢复已激活 Skill。
  - 新增 `RunnerPool` 有独立单元测试覆盖创建、复用、关闭、busy 检测。

- **负面**
  - Session 文件体积会因每次 Skill 激活（含重复激活）而增大，但在 MVP 可接受。
  - `SessionManager::delete_session` 与文件删除之间的原子性较原先略有弱化（原先持有 `runners` map 锁跨越删除），但通过 `close` 先移除 runner 再删除文件仍能保证不删除 active run 的 session。
  - 需要修正 `docs/adr/0011-split-tools-skills-registries.md` 中关于激活状态不持久化的负面说明。
