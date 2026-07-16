# Research: Automatic Compaction Entry

**Feature**: specs/002-auto-compaction-entry  
**Date**: 2026-07-16  
**Purpose**: Resolve technical unknowns before design and contract generation.

## Unknowns Resolved

### 1. How to measure "active Message History" against the Context Budget

**Decision**: 在 `byte-core` 中维护一个 active-path token estimator，在每次发送给 Model Provider 之前对当前 active path 进行 token 估算。估算逻辑优先复用 Model Provider 配置中已暴露的 tokenizer（若未来支持），MVP 阶段使用与 `byte-models` 一致的近似算法（如按字符/词元估算），确保触发阈值稳定、可测试。

**Rationale**: Context Budget 是模型相关概念，但触发决策必须在 daemon 本地完成；近似估算足够满足 90% 触发阈值，且不会引入外部网络依赖。

**Alternatives considered**: 在每次模型调用失败后再触发（过于被动，会破坏用户体验）；完全依赖模型返回的 token 统计（无法在发送前决策）。

### 2. How to generate the Compaction Entry summary

**Decision**: 通过 `byte-models` 的 provider 抽象向同一 Model Provider 发送一次 summarization 请求。提示词要求模型总结被压缩消息块中的开发者意图、助手决策、工具结果，并以结构化摘要返回。摘要长度由实现层控制，默认不超过上下文预算的固定比例（例如 20%）。

**Rationale**: 与规格中“模型生成摘要”的选择一致；复用现有 provider 抽象避免新增依赖；摘要质量直接影响 SC-003 的继续推理能力。

**Alternatives considered**: 本地规则压缩（会丢失语义）；混合模式（MVP 先保持简单，可后续引入）。

### 3. How to represent the Compaction Entry in the persisted Session tree

**Decision**: 在 `byte-session` 的 JSONL 树中新增一种节点类型 `CompactionEntryNode`，包含 `summary`、`compacted_range`（被压缩消息在树中的 ID 范围或列表）、`created_at`。原始消息节点保留不动，Active Path 重建逻辑通过识别 CompactionEntryNode 来跳过被压缩消息。

**Rationale**: 满足 FR-004（不删除原始消息）和 FR-005（用 Compaction Entry 替代旧消息）；向后兼容旧 Session 可通过在加载时忽略未知节点类型或将其视为普通文本节点实现。

**Alternatives considered**: 将 Compaction Entry 作为独立 sidecar 文件（会增加持久化复杂度）；将摘要嵌入每个被压缩消息节点（会破坏 Active Path 的线性视图）。

### 4. What runtime events the UI needs to observe compaction

**Decision**: 在 `byte-protocol` 中定义三个事件：`CompactionStarted`、`CompactionCompleted`、`CompactionFailed`。事件包含 `session_id` 和 `compaction_entry_id`（或临时 id），用于前端更新时间线状态。

**Rationale**: 桌面端需要感知 compaction 进度以渲染加载/展开状态；事件属于 ephemeral projection，与 PR-004 一致。

**Alternatives considered**: 让前端轮询 daemon 状态（增加复杂度且不符合现有事件驱动模型）；仅在 Session 重载后显示 Compaction Entry（无法展示实时状态）。

### 5. How to prevent infinite compaction loops

**Decision**: 在创建 Compaction Entry 后，如果新的 active path 仍然超过 90% 预算，则停止再次压缩，直接失败该 Run 并返回清晰错误。实现层通过记录“最近一次尝试创建的摘要”来避免重复压缩同一段历史。

**Rationale**: 覆盖 Edge Case “summary itself is large enough”；与 FR-010 一致。

**Alternatives considered**: 递归压缩直到预算足够（可能导致无限循环或摘要质量极差）；仅对摘要本身再次压缩（实现复杂且收益低）。

### 6. Backward compatibility for Sessions without Compaction Entries

**Decision**: `byte-session` 加载旧 Session JSONL 时，对未知节点类型使用容错策略：默认忽略或降级为普通文本节点。新增 `format_version` 字段；若版本低于当前，则以只读兼容模式加载，保存时自动升级到新版本。

**Rationale**: 满足 PR-003；旧 Session 不应因新类型而无法加载。

**Alternatives considered**: 强制迁移脚本（MVP 期间用户手动启动 daemon，自动升级更轻量）。

## Research Output Status

- [x] Token estimation approach: 复用/近似 tokenizer，本地触发
- [x] Summarization approach: 通过 `byte-models` 调用同一 provider
- [x] Persistence representation: JSONL 树新增 `CompactionEntryNode`
- [x] Runtime events: `CompactionStarted` / `CompactionCompleted` / `CompactionFailed`
- [x] Infinite loop prevention: 一次压缩后仍超预算则失败
- [x] Backward compatibility: 未知节点降级/自动升级
