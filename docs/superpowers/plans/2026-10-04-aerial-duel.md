# Aerial PvP implementation plan

**Goal:** Run two teams in the existing local Java 1.21.11 arena using mace
wind-charge/pearl attacks and spear/elytra/firework return passes.

**Architecture:** Deterministic, observable tactical controllers consume live
positions and inventory, and issue normal movement, use-item and attack inputs.
Vanilla controls damage, teleports and knockback. This is scripted combat AI,
not a claim that a trained model learned PvP. Start at five accounts per team.

**Constraints:** Preserve fist combat. No active-combat server teleports, damage
commands, fake hits or speed attributes. Finite survival equipment. Budget is
256,000,000 RSS bytes per bot group and must be measured separately from server.
Do not alter the user's player data or other servers.

## Work boundaries

- Command/runtime: `duel mace|spear <enemy-prefix>`, task lifecycle and plugin
  registration. Prefix targets only live visible opposing players, including
  accounts in a shared ECS. `stop`, death and disconnect cancel item use.
- Aerial physics: supply missing gliding travel and actual attached-firework
  acceleration, with pure numerical tests and narrow local-only integration.
- Tactics: inventory-synchronized actions; launch, acquire, strike, recover for
  mace; takeoff, accelerate, charge, exit, turn, reacquire for spear. Track target
  motion every tick. Stop safely if equipment is missing or target disappears.
- Arena: reproducible setup for two tagged teams, normal equipment, controls,
  and server-backed damage evidence. Keep setup separate from fighting.

## Validation sequence

1. Parser and state-transition failing tests, then implementation.
2. Flight numerical/attachment tests and existing fist protocol regressions.
3. Workspace tests, formatting, Clippy and release build.
4. Live one-versus-one trials establish launch/gliding and actual damage.
5. Five-versus-five trial records server health/death and tactical state events,
   memory and disconnects. Document exactly which combinations were observed.
