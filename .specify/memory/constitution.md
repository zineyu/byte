<!--
Sync Impact Report
- Version change: unratified template → 1.0.0
- Modified principles: none (newly ratified)
- Added principles:
  I. Shared Protocol and Dependency Direction
  II. Daemon-Owned Runtime, Thin Clients
  III. Explicit Trusted-Local Security Posture
  IV. Verifiable Runtime and Contract Correctness
  V. Domain and Documentation Fidelity
- Added sections:
  - Project Constraints (Security & Risk Acceptance; Design Token Authority)
  - Development Workflow & Quality Gates
  - Governance (Amendment Procedure, Versioning Policy, Compliance Review)
- Removed sections: none
- Templates requiring updates:
  - .specify/templates/plan-template.md ✅ updated (concrete Constitution Check gates)
  - .specify/templates/spec-template.md ✅ updated (Security/Risk + Protocol/Compatibility requirements)
  - .specify/templates/tasks-template.md ✅ updated (test requirements for runtime/protocol/tool changes)
  - .pi/prompts/speckit.*.md ✅ checked, no outdated references
  - .omp/commands/speckit.*.md ✅ checked, no outdated references
- Runtime guidance docs updated:
  - README.md ✅ updated (manual daemon start, WebSocket JSON-RPC, crate descriptions, dependency diagram)
  - AGENTS.md ✅ updated (manual daemon start, WebSocket JSON-RPC, crate descriptions, dependency diagram)
- Follow-up TODOs: none
-->

# Byte Agent Constitution

## Core Principles

### I. Shared Protocol and Dependency Direction

All cross-boundary types, JSON-RPC commands, runtime events, session view types, and
daemon address validation MUST live in `byte-protocol` and MUST NOT be redefined in
`byte-daemon`, `apps/desktop/src-tauri`, or the React frontend.

The dependency graph MUST be:

- `byte-daemon` → `byte-core` → `byte-tools`, `byte-skills`, `byte-models`, `byte-session`
- `byte-tools`, `byte-skills`, `byte-models`, `byte-session`, `byte-daemon`,
  `apps/desktop/src-tauri` → `byte-protocol`
- No reverse dependencies.

Rationale: A single protocol crate prevents type drift between the Rust daemon and
TypeScript frontend, keeps the UI layer replaceable, and makes the runtime testable in
isolation.

### II. Daemon-Owned Runtime, Thin Clients

The Rust daemon MUST own the JSON-RPC command surface, runtime state management,
model/tool loop execution, and session persistence.

The desktop shell MUST be a client only: it MAY connect to a manually-started daemon,
maintain the WebSocket transport, expose Tauri commands for UI actions, and forward
runtime events to React. It MUST NOT spawn, own, or kill the daemon process, and it MUST
NOT implement model loops, tool execution, or business logic.

Rationale: Decoupling the daemon lifecycle from the desktop app lets multiple clients share
the same runtime and allows the daemon to survive UI restarts.

### III. Explicit Trusted-Local Security Posture

The MVP MUST run in unrestricted local agent mode: file read/write, command execution,
deletion, and network-capable commands are permitted without runtime permission filtering,
but only within a Code Workspace the Developer has intentionally opened in a trusted local
environment.

The daemon is started manually by the user and listens only on `127.0.0.1` or `localhost`.
The desktop shell MUST reject non-local daemon addresses.

API keys and provider secrets MAY be stored in plaintext in local config files during the
MVP, as documented in ADR-0016. Real secrets MUST NOT be committed to the repository.

Rationale: Security scope is intentionally reduced for MVP velocity while preserving
documented seams for future policy, keychain, and sandboxing.

### IV. Verifiable Runtime and Contract Correctness

Rust code MUST have tests covering JSON-RPC framing, request/response correlation, runtime
event ordering, session append and active-path reconstruction, `apply_patch` behavior,
`run_command` streaming, and skill discovery/collision precedence.

Every bug fix or change to a protocol contract, tool behavior, session persistence, or runtime
event MUST include a regression or contract test.

Frontend code MUST pass typecheck and build before merge.

Rationale: The daemon is the source of truth for session state; automated tests are the only
practical way to guarantee correctness across the tool loop.

### V. Domain and Documentation Fidelity

When naming a domain concept defined in `CONTEXT.md`, all issues, specifications,
tests, ADRs, and code comments MUST use that canonical term. Terms with explicit
avoided synonyms MUST NOT be replaced by those synonyms.

New domain terms MUST be added to `CONTEXT.md` before being used in specs or tests.

Documentation (`README.md`, `AGENTS.md`, `DESIGN.md`, `CONTEXT.md`, `docs/adr/`, and
`docs/agents/`) MUST be updated when the corresponding code, design, or domain language
changes.

Rationale: Shared language and documentation keep the codebase navigable for humans and agents
and prevent silent drift between architecture decisions and implementation.

## Project Constraints

### Security & Risk Acceptance

- The MVP runs in unrestricted local agent mode and accepts the risks of file damage, secret
  leakage, and unintended command execution as documented in ADR-0004 and the README security
  notice.
- Plaintext API key storage is accepted during the MVP; a secret seam will be introduced only
  when OS keychain support is planned.
- Workspace Skills may inject strong behavioral instructions; skill content is treated as
  part of the trusted local environment.

### Design Token Authority

- All UI colors, fonts, radii, spacing, and component styles in `apps/desktop` MUST come from
  `DESIGN.md`.
- Any frontend style, layout, or component change MUST be reflected back in `DESIGN.md`.
- New colors, font stacks, or radius systems MUST NOT be introduced in components or style
  sheets.
- `DESIGN.md` MUST pass `npx @google/design.md lint DESIGN.md` with no errors before merge;
  warnings MUST be documented as intentional design choices.

Rationale: Keeps the interface coherent and makes the design system lintable against the
actual UI.

## Development Workflow & Quality Gates

- Use `jj` for version control; direct `git` commands MUST NOT be used.
- Enter the development environment via `devenv shell` or `direnv allow`.
- Every change MUST pass `just verify` before merge.
- Multi-file, cross-module, interface, permission, state-flow, async, or concurrent changes
  MUST be proposed with scope, risks, and acceptance criteria before implementation begins.
- Issues and PRDs MUST be tracked in GitHub Issues for `zineyu/byte` using the triage labels
  defined in `docs/agents/triage-labels.md`.

## Governance

This constitution supersedes ad-hoc conventions in this repository.

### Amendment Procedure

1. Propose the change, affected principles, rationale, and affected files.
2. Update `.specify/memory/constitution.md`.
3. Synchronize `.specify/templates/plan-template.md`, `spec-template.md`, `tasks-template.md`,
   and installed `speckit.*` command files if any principle changes the mandatory gates or task
   categories.
4. Update runtime guidance docs (`README.md`, `AGENTS.md`, etc.) if any principle changes the
   workflow or architecture.
5. Pass `just verify repo` or equivalent repository checks before finalizing.

### Versioning Policy

Constitution versions follow semantic versioning:

- MAJOR: backward-incompatible governance or principle removals/redefinitions.
- MINOR: new principle/section added or materially expanded guidance.
- PATCH: clarifications, wording fixes, typo fixes, or non-semantic refinements.

### Compliance Review

Every PR or review MUST verify that the change does not violate a constitution principle.
If a violation is intentional, it MUST be justified in the PR description and recorded as an
ADR or constitution amendment.

**Version**: 1.0.0 | **Ratified**: 2026-07-16 | **Last Amended**: 2026-07-16
