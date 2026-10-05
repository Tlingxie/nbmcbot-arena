# NBMCBot Runtime Implementation Plan

> For agentic workers: use subagent-driven-development to implement the independent tasks below, with tests and integration review.

Goal: deliver an executable Rust Minecraft bot with commands, navigation,
ported automation, bounded Wasm plugins, and reproducible memory measurements.

Architecture: one Azalea-backed game session owns the world. Small native
modules and Wasm addons submit actions through its control loop. A separate
core crate supplies strict input parsing and explicit resource accounting.

Tech stack: Rust, Azalea 0.15.1+mc1.21.11, wasmi 2.0.0,
Tokio, serde, TOML, and a local protocol test fixture.

Spec: ../specs/2026-10-04-nbmcbot-rewrite-design.md

## Global constraints

- Total target is 256,000,000 bytes RSS, including required helper processes.
- User has approved implementation and unrestricted source/addon changes.
- Preserve the full-feature goal; unfinished ports remain explicitly incomplete.
- No unmodified JAR compatibility claim and no fabricated memory results.
- One pinned Java Edition version first; protocol/runtime behavior is tested.
- Work on codex/nbmcbot in this otherwise empty repository; no existing code is displaced.
- Write code with apply_patch, retain lockfiles, and use release builds for measurements.

## Task 1: Core contracts and memory accounting

Files: crates/nbmcbot-core/Cargo.toml, crates/nbmcbot-core/src/*.rs.

Produces Command, parse_command, MemoryBudget, MemoryLease, and process_memory.
Command parsing must reject invalid, infinite, missing, and out-of-range
arguments. Memory reservations must not overcommit, leak on failure, or retain
charges after their ownership ends. Real OS RSS is reported separately from
accounted allocations; bookkeeping alone is not a process cap.

- [x] Write parser and budget boundary tests.
- [x] Implement the isolated core crate and run its tests.
- [x] Exercise real RSS sampling on this host and document units.

## Task 2: Bounded plugin runtime

Files: crates/nbmcbot-plugins/Cargo.toml, crates/nbmcbot-plugins/src/*.rs,
examples/plugins/*.

Produces PluginRuntime, PluginLimits, PluginAction, and BotSnapshot.
Loads Wasm/WAT packages with explicit imports and exports. Enforces file size,
linear memory, tables, instance count, instruction fuel, host action queue,
and aggregate admission limits. Trapped callbacks cannot commit actions.
Root integration consumes returned actions, never direct game mutation.

- [x] Test a plugin emitting chat and navigation actions through the real engine.
- [x] Test loops, memory growth, malformed imports, oversized files, and action flooding.
- [x] Implement runtime and runnable examples; verify configured limits and lifecycle cleanup.

## Task 3: Executable game session

Files: Cargo.toml, rust-toolchain.toml, crates/nbmcbot/Cargo.toml,
crates/nbmcbot/src/*.rs, config.example.toml.

Consumes core command parsing and plugin actions. Produces nbmcbot executable
with explicit offline/Microsoft authentication modes, stdin command control,
status, chat, navigation, block interaction, inventory inspection, modules,
plugin loading, cancellation, disconnect, and bounded reconnection.

- [x] Pin a published Azalea version and its matching Rust toolchain.
- [x] Add configuration and command-dispatch tests, including a failing protocol baseline before runtime integration.
- [x] Integrate one shared game world and actual upstream control APIs.
- [x] Implement useful native automation with independent policy tests.
- [x] Ensure disconnect invalidates tasks and quit terminates the tested session.

## Task 4: Runtime verification and packaging

Files: crates/nbmcbot/tests/*, scripts/*, README.md, docs/FEATURES.md,
docs/VALIDATION.md, .gitignore.

- [x] Run formatting, workspace tests, and clippy; repair concrete failures.
- [x] Exercise actual local protocol login and observable command effects.
- [x] Build release binaries and measure startup/session/plugin memory.
- [x] Record exactly tested conditions and remaining feature/validation gaps.
- [x] Perform independent code review, fix findings, and repeat affected tests.

## Progress and rulings

- Research and implementation started after user approval on 2026-10-04.
- User selected Minecraft 1.21.11. Pin Azalea 0.15.1+mc1.21.11 rather than
  following its current development branch.
- Full Meteor/Baritone parity requires release-specific ports and acceptance
  cases. Do not equate this runtime milestone with complete original scope.
- Runtime milestone verified: 37 workspace tests, formatting and Clippy pass;
  release fixture workload plus 60 seconds of plugin activity peaks at
  33,095,680 bytes RSS. See ../../VALIDATION.md for exact evidence and limits.
- The full original goal remains open: the upstream inventory is unverified,
  real Meteor addon ports and visual systems are absent, and there is no
  validated whole-workload 256 MB hard-bound implementation or 24-hour soak.
