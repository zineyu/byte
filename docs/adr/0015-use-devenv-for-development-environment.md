# ADR-0015: 使用 devenv 替代 flake.nix 管理开发环境

## Status

Accepted

## Date

2026-07-13

## Context

Byte Agent 的开发环境由 Nix 管理，原方案使用根目录 `flake.nix` 声明 Rust、Node.js、Tauri 系统依赖及环境变量。`direnv` 通过 `.envrc` 中的 `use flake` 自动加载开发环境。

现有 `flake.nix` 提供了完整的环境，但存在以下问题：
- 开发环境配置与项目 Nix flake 输入耦合，Nix 语法对于非 Nix 用户较重；
- 缺乏针对开发流程的声明式原语（如 `languages.*`、`env`、`enterShell`）；
- 项目 instructions 已明确使用 `nix + direnv + devenv`，需要工具链与约定保持一致。

## Decision

使用 [devenv](https://devenv.sh) 替代 `flake.nix` 作为开发环境管理工具，保留 `nix` 和 `direnv` 不变。

具体变更：
- 新增 `devenv.nix`：通过 `rust-overlay` 安装与原 `flake.nix` 一致的 Rust 工具链（`rust-bin.stable.latest.default` + `rust-src`/`rustfmt`/`clippy`），声明 Node.js 22、Tauri 系统依赖、环境变量与 `enterShell`；
- 新增 `devenv.yaml`：声明 `nixpkgs` 和 `rust-overlay` 输入；
- 新增 `devenv.lock`：锁定输入版本，保证环境可复现；
- 更新 `.envrc`：从 `use flake` 改为 `use devenv`；
- 删除 `flake.nix` 和 `flake.lock`；
- 更新 `.gitignore` 和 `docs/operations/commands.md`。

## Alternatives Considered

### 保留 flake.nix 并添加 devenv-flake 集成

- 保留 `flake.nix`，在其中使用 `devenv.lib.mkShell` 生成开发环境；
- 优点：兼容现有 Nix flake 生态，可直接用 `nix develop`；
- 缺点：仍然维护 `flake.nix`，且没有真正切换到 devenv 的声明式开发环境原语；与项目 instructions 中“使用 devenv”不完全一致。
- 结论：拒绝，目标是完全替代 flake.nix，简化项目入口。

### 使用纯 nixpkgs 而不引入 rust-overlay

- 在 `devenv.nix` 中不声明 `rust-overlay` 输入，直接通过 `pkgs.rustc`/`cargo` 等安装 Rust；
- 优点：少一个输入，锁文件更轻；
- 缺点：无法精确控制 Rust 组件和版本，且 devenv 的 `languages.rust` 默认依赖 rust-overlay；
- 结论：拒绝，rust-overlay 能稳定提供项目所需的 stable Rust + rust-src + rustfmt + clippy。

## Consequences

- 开发环境入口变为 `devenv shell` 或 `direnv allow`（自动加载）；
- 环境变量、激活脚本和包列表集中在 `devenv.nix` 中，便于阅读和增量调整；
- 失去 `nix develop` 和 `nix build` 入口；CI 和 `just` 任务不再依赖 Nix flake；
- 需要所有贡献者安装 `devenv`（≥ 2.0）以使用 `use devenv` 的 `.envrc`；
- 未来如需引入数据库、缓存等服务，可直接使用 devenv 的 `services.*` 模块扩展。
