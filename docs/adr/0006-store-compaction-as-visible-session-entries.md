# Store compaction as visible session entries

Byte Agent will represent automatic compaction results as explicit `Message` entries with `role = "summary"` inside the Session JSONL tree. Context construction may use these summaries in place of older active-path messages, while the original entries remain preserved for inspection and branching; this keeps long-session behavior explainable instead of hiding summaries in an opaque cache.
