# Shared Swarm Implementation Plan

> Agent workers implement independent owned files and report verification to root.

**Goal:** Run ten same-server bots in one process, prove chunk sharing, and measure memory.

**Architecture:** One Azalea Swarm and ECS share chunk bodies. Each bot has an
independent Session; process shutdown and RSS monitoring belong to the group.

**Tech Stack:** Rust nightly-2026-03-01, pinned Azalea 0.15.1 / Minecraft 1.21.11,
Wasm plugins, Node measurement scripts.

**Spec:** `docs/superpowers/specs/2026-10-04-shared-swarm-design.md`

## Global constraints

- One server per group; 1..32 unique account names.
- Preserve the existing single-bot CLI and tests.
- A 256,000,000-byte default RSS budget covers the whole shared process.
- Do not claim full Meteor/Baritone parity or unchanged JAR compatibility.

## Runtime and isolation (core_runtime)

Files: `crates/nbmcbot/src/runtime.rs`, `crates/nbmcbot/src/game.rs`.

- [x] Write and run failing entity-isolation regressions.
- [x] Add `FleetCommand::{All(Command), Bot { username, command }}` and
  `run_swarm(Config, Vec<String>, Receiver<FleetCommand>) -> Result<()>`.
- [x] Scope queued-action cancellation and reconnect state to the target entity.
- [x] Label bot events; report real unique chunk/world Arc counts in group status.
- [x] Run runtime/game tests using `sh scripts/cargo.sh test --locked -p nbmcbot --lib`.

## Same-server fixture and integration (plugins)

Files: `tests/support/mod.rs`, `tests/swarm_e2e.rs`, `examples/fixture_server.rs`.

- [x] Add a failing two-bot CLI integration test before the CLI exists.
- [x] Support concurrent clients on one listener with distinct identities.
- [x] Add `--clients N` and `--chunk-radius R` to the fixture executable.
- [x] Verify actual Arc sharing, targeted cancellation, disconnect and reconnect.
- [x] Preserve the original single-bot protocol integration tests.

## CLI, measurement and documentation (root)

Files: `src/main.rs`, `src/input.rs`, `scripts/measure-shared.mjs`, `README.md`,
`docs/VALIDATION.md` (crate paths above are relative to `crates/nbmcbot`).

- [x] Test command routing rejection/selection, then implement `swarm` and `@name`.
- [x] Measure both layouts with the same server fixture, chunk radius and workload.
- [x] Record all connected accounts, navigation completion, plugin events and
  shutdown success alongside 100 ms RSS samples and OS process peaks.
- [x] Run workspace tests, fmt, Clippy, release build and the actual comparison.
- [x] Review isolation and measurement methodology; fix substantive findings.
- [x] Report measured totals and remaining fixture/workload limitations.

## Result

Ten independent processes averaged 372.45 MB RSS; one shared process with ten
accounts averaged 63.54 MB with a 63.65 MB OS peak. Verified 250 chunk references
to 25 unique chunk bodies. Workspace: 43 tests; client patch regressions: 4 tests.
Detailed workload, raw reports, the rejected startup attempt and scope limits
are recorded in `docs/VALIDATION.md`.
