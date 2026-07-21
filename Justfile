set shell := ["bash", "-euo", "pipefail", "-c"]

# Run validation suite or one gate: just verify [all|repo|design-md|workflow|rust|desktop|audit].
verify target="all":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ target }}" in
      all)
        just _repo-hygiene
        just _design-md-check
        just _workflow-syntax
        just _verify-rust
        just _verify-desktop
        just _verify-audit
        ;;
      repo|repo-hygiene)
        just _repo-hygiene
        just _design-md-check
        ;;
      design-md|design.md)
        just _design-md-check
        ;;
      workflow|workflow-syntax)
        just _workflow-syntax
        ;;
      rust)
        just _verify-rust
        ;;
      desktop)
        just _verify-desktop
        ;;
      audit|audi)
        just _verify-audit
        ;;
      *)
        echo "unknown verify target: {{ target }}" >&2
        echo "usage: just verify [all|repo|design-md|workflow|rust|desktop|audit]" >&2
        exit 2
        ;;
    esac

# Validate DESIGN.md against the Google Labs design.md specification.
_design-md-check:
    #!/usr/bin/env bash
    set -euo pipefail
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT
    npm install --prefix "$tmpdir" @google/design.md >/dev/null 2>&1
    "$tmpdir/node_modules/.bin/designmd" lint DESIGN.md

# Check required docs, Markdown sanity, and obvious committed secrets.
_repo-hygiene:
    #!/usr/bin/env bash
    set -euo pipefail

    required_files=(
      AGENTS.md
      CONTEXT.md
      docs/architecture/mvp-architecture.md
      docs/agents/domain.md
      docs/agents/issue-tracker.md
      docs/agents/triage-labels.md
    )

    for file in "${required_files[@]}"; do
      if [ ! -s "$file" ]; then
        echo "Required documentation file is missing or empty: $file" >&2
        exit 1
      fi
    done

    adr_count="$(find docs/adr -maxdepth 1 -type f -name '*.md' | wc -l | tr -d ' ')"
    if [ "$adr_count" -lt 7 ]; then
      echo "Expected at least 7 ADR files, found $adr_count" >&2
      exit 1
    fi

    found=0
    while IFS= read -r -d '' file; do
      found=1
      if [ ! -s "$file" ]; then
        echo "Markdown file is empty: $file" >&2
        exit 1
      fi
      if grep -n $'\r' "$file" >/tmp/byte-crlf-lines.txt; then
        echo "Markdown file contains CRLF line endings: $file" >&2
        cat /tmp/byte-crlf-lines.txt >&2
        exit 1
      fi
    done < <(find . \
      -path './.git' -prune -o \
      -path './.jj' -prune -o \
      -path './target' -prune -o \
      -path './apps/desktop/node_modules' -prune -o \
      -path './apps/desktop/dist' -prune -o \
      -path './apps/desktop/src-tauri/target' -prune -o \
      -type f -name '*.md' -print0)

    if [ "$found" -eq 0 ]; then
      echo "No Markdown files found" >&2
      exit 1
    fi

    if grep -RInE \
      --exclude-dir=.git \
      --exclude-dir=.jj \
      --exclude-dir=target \
      --exclude-dir=node_modules \
      --exclude-dir=dist \
      --exclude='*.lock' \
      '(OPENAI_API_KEY|ANTHROPIC_API_KEY|AWS_SECRET_ACCESS_KEY|SECRET_KEY|PRIVATE_KEY|GITHUB_TOKEN)=[^[:space:]]+' \
      .; then
      echo "Potential committed secret detected" >&2
      exit 1
    fi

# Parse GitHub Actions workflow YAML files.
_workflow-syntax:
    ruby -e 'require "yaml"; Dir[".github/workflows/*.{yml,yaml}"].sort.each { |file| YAML.load_file(file); puts "parsed #{file}" }'

# Format all Rust and desktop frontend code.
fmt: _desktop-install
    cargo fmt --all
    cd apps/desktop && pnpm run fmt

# Check all Rust and desktop frontend formatting without modifying files.
fmt-check: _rust-fmt-check _desktop-fmt-check

_verify-rust: _rust-fmt-check _rust-clippy _ts-export-check _rust-test

_rust-fmt-check:
    cargo fmt --all -- --check

# Regenerate ts-rs bindings from byte-protocol and fail if the committed
# TypeScript files under apps/desktop/src/generated have drifted. Must run
# before any other cargo test invocation refreshes the bindings in place.
_ts-export-check:
    #!/usr/bin/env bash
    set -euo pipefail
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT
    cp -r apps/desktop/src/generated "$tmpdir/generated"
    cargo test -p byte-protocol --quiet
    if ! diff -r "$tmpdir/generated" apps/desktop/src/generated; then
      echo "ts-rs generated bindings are out of sync with byte-protocol." >&2
      echo "The re-exported files are left in apps/desktop/src/generated; commit them." >&2
      exit 1
    fi

_rust-clippy:
    cargo clippy --workspace --all-targets -- -D warnings

_rust-test:
    cargo test --workspace --all-targets

# Install desktop dependencies exactly as CI does when pnpm is unavailable;
# prefer an existing local pnpm binary to support immutable Nix environments.
_desktop-install:
    #!/usr/bin/env bash
    set -euo pipefail
    cd apps/desktop
    if command -v pnpm >/dev/null 2>&1; then
      pnpm install --frozen-lockfile
    elif command -v corepack >/dev/null 2>&1; then
      # In read-only Nix environments, corepack enable/prepare --activate fails
      # because it tries to write into the Node.js installation directory. Use
      # corepack pnpm directly instead; it respects the packageManager field
      # and caches pnpm in a writable location.
      corepack pnpm install --frozen-lockfile
    else
      echo "pnpm not found and corepack is unavailable; please install pnpm" >&2
      exit 1
    fi

_verify-desktop: _desktop-fmt-check
    #!/usr/bin/env bash
    set -euo pipefail
    cd apps/desktop

    run_script() {
      local script="$1"
      if node -e "const scripts = require('./package.json').scripts || {}; process.exit(scripts[process.argv[1]] ? 0 : 1)" "$script"; then
        pnpm run "$script"
      else
        echo "No package.json script named '$script'; skipping."
      fi
    }

    run_script lint
    run_script typecheck
    run_script test
    run_script build

_desktop-fmt-check: _desktop-install
    cd apps/desktop && pnpm run fmt:check

_verify-audit: _desktop-install
    cd apps/desktop && pnpm audit --audit-level high

# Build the local daemon used by the desktop app.
build-daemon:
    cargo build -p byte-daemon

# Start the local daemon on a loopback WebSocket address (default: 127.0.0.1:8787).
start-daemon addr="127.0.0.1:8787": build-daemon
    ./target/debug/byte-daemon --rpc-websocket {{ addr }}

# Start the desktop app in development mode.
# NOTE: start the daemon first in another terminal with `just start-daemon`.
start-desktop: _desktop-install
    @echo "Reminder: run \`just start-daemon\` in another terminal first."
    cd apps/desktop && pnpm run tauri:dev
