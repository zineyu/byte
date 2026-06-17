# Issue tracker: GitHub

Issues and PRDs for this repo live as GitHub issues for `zineyu/byte`. Use the `gh` CLI for issue operations.

## Conventions

- **Create an issue**: `gh issue create --title "..." --body "..."`. Use a heredoc for multi-line bodies.
- **Read an issue**: `gh issue view <number> --comments`, filtering comments by `jq` and also fetching labels.
- **List issues**: `gh issue list --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'` with appropriate `--label` and `--state` filters.
- **Comment on an issue**: `gh issue comment <number> --body "..."`
- **Apply / remove labels**: `gh issue edit <number> --add-label "..."` / `--remove-label "..."`
- **Close**: `gh issue close <number> --comment "..."`

This repo is also a Jujutsu workspace. Do not run direct `git` commands here; use `jj` for VCS operations. `gh` can usually infer the repo from the clone, and the configured origin is `git@github.com:zineyu/byte.git`.

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Run `gh issue view <number> --comments`.

## After closing an agent-implemented issue

完成实现并通过验证后，建议更新关联 issue 并关闭：

1. 在该 issue 下评论说明实现范围、验证结果和关键文件。
2. 如果 issue 启用了验收清单，勾选已完成的条目。
3. 关闭 issue 时引用本次变更涉及的主要文件或测试。

示例关闭评论：

> 已实现 issue #1 的最小骨架：
> - `crates/byte-protocol` 与 `crates/byte-daemon` 提供 stdio JSONL JSON-RPC 与 `get_state`。
> - `apps/desktop` 完成 Tauri v2 + React 桌面壳，可启动 daemon 并显示连接状态。
> - 验证：`cargo test`、`pnpm run typecheck/build`、`pnpm audit` 均通过。
