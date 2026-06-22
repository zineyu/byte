# Development Commands

Common commands for building, testing, and running Byte Agent.

## Rust workspace

```bash
# Build the workspace
cargo build

# Run daemon integration tests
cargo test -p byte-daemon --test unix_socket_json_rpc

# Formatting / linting / tests
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

## Desktop application

```bash
cd apps/desktop

# Install dependencies
pnpm install

# Type check and build the frontend
pnpm run typecheck
pnpm run build

# Run in development mode (builds daemon then starts Tauri)
pnpm run tauri:dev

# Dependency audit
pnpm audit --audit-level high
```

Note: on the current development machine, Tauri needs the dynamic loader path:

```bash
LD_LIBRARY_PATH=/usr/lib pnpm run tauri:dev
```
