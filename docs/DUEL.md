# Mace arena: team battles and player siege

The local Java 1.21.11 arena is `127.0.0.1:25566`. This scenario uses two shared
bot processes, with five accounts each by default: `Mace001` through `Mace005`
and `Spear001` through `Spear005`. It uses the existing official server and a
release binary with the `duel` command. Server memory is separate from bot RSS;
the 256 MB limit applies to each bot process, not the sum of both processes.

Start the server and groups, then wait for every account to spawn:

```sh
node scripts/arena-process.mjs server start
node scripts/arena-process.mjs mace-team start
node scripts/arena-process.mjs spear-team start
node scripts/duel-arena.mjs setup
node scripts/duel-arena.mjs fight
```

Skip `start` for a process that is already running. Set `NBMCBOT_DUEL_COUNT`
when starting each group to choose 1 through 50 accounts per side, for example
`NBMCBOT_DUEL_COUNT=2 node scripts/arena-process.mjs mace-team start`. Subsequent
arena commands use account names saved in each running group's process record.
The count is not a promise that every workload will fit the memory budget.

The web viewer offers `mace-vs-mace`, `mace-vs-spear`, and `mace-vs-player`
(all bots attack the selected online human). Its total `botCount` is 1–100,
default 10. The viewer requires at least two bots for a two-team mode; player
siege permits one. The viewer rebuilds the groups to contain `ceil(total / 2)`
Mace accounts and `floor(total / 2)` Spear accounts. With one bot, the second group
is not started or required.

For script use, set `NBMCBOT_DUEL_TOTAL` to the same total and, for siege,
set a strict Minecraft username in `NBMCBOT_DUEL_PLAYER`:

```sh
NBMCBOT_DUEL_MODE=mace-vs-player NBMCBOT_DUEL_TOTAL=10 NBMCBOT_DUEL_PLAYER=PlayerName node scripts/duel-arena.mjs setup
NBMCBOT_DUEL_MODE=mace-vs-player NBMCBOT_DUEL_TOTAL=10 NBMCBOT_DUEL_PLAYER=PlayerName node scripts/duel-arena.mjs fight
```

These commands do not start, resize, or stop group processes. Prepare the
matching running groups first; explicit totals are checked against their saved
account records. Without `NBMCBOT_DUEL_TOTAL`, existing record counts remain
compatible, and dry runs use the legacy `NBMCBOT_DUEL_COUNT` (default five per
group). `NBMCBOT_DUEL_MODE` defaults to `mace-vs-spear`.

Both duel groups start with `auto-respawn` enabled through their generated startup configuration, so dead accounts can reconnect before setup commands arrive; other bot groups keep their existing defaults.

`setup` enables `auto-respawn`, cancels both groups' tasks, then waits up to ten
seconds for a fresh status from every named test account showing that it is
connected, has positive health, and has no active task. A dead account must
respawn through the normal client/server flow before setup proceeds; healing
commands are not used to simulate resurrection. If any account is still dead,
disconnected, or missing a fresh status, setup fails without sending healing,
equipment, or teleport commands. Auto-respawn remains enabled while preparing
the next round and is disabled by `fight` before either group is armed.

In the two-team modes, setup only changes the named test accounts, gives them the `nbmc_duel` tag,
joins the `nbmc_mace` and `nbmc_spear`
teams, and disables friendly fire. It clears their inventories and effects,
restores health and food once, equips them, sets survival mode and respawn
points, and places the teams at X = -12 and X = 12, Y = 64, with three blocks
between accounts along Z. Repeating setup repositions and replenishes the test
accounts; do this only when resetting a round. An existing-team notice from
`team add` on later setups is harmless. No player outside the named test groups
is equipped, teleported or assigned to a team.

The mace team starts with netherite helmet/leggings/boots and an elytra. Its
hotbar contains a mace in slot 0, 64 wind charges in slot 1, 16 ender pearls in
slot 2, and a spare netherite chestplate in slot 3. It also carries 64 Flight 1
firework rockets in its offhand. The spear team keeps its netherite
helmet/leggings/boots, elytra, netherite spear in slot 0, and 64 Flight 1
firework rockets in its offhand. Both teams use a defensive enchanted kit:
Protection IV on every netherite armor piece, including the mace team's spare
chestplate, and Feather Falling IV on the boots. Elytras have no Protection
enchantment. Rockets have no explosion
payload. Equipment is unbreakable; bot maces carry Wind Burst III, the highest
legal level (`max_level: 3`) in the official 1.21.11 enchantment data. Spears
remain unenchanted, and there are no
custom health, damage, speed, or other attribute modifiers. These standard
vanilla defensive enchantments reduce early one-hit kills and fall damage so
longer exchanges can be observed; they do not guarantee survival of every hit.
Consumables are finite. The default measured round has no teleports, health
resets or replenishment commands during active combat. To explicitly supply
more rockets without resetting a round, run:

```sh
node scripts/duel-arena.mjs resupply
```

In `mace-vs-player`, `ceil(total / 2)` bots use Wind Burst III maces and
`floor(total / 2)` bots use spears, elytras and rockets. For an odd total,
the mace group has one extra bot. Both groups join the same `nbmc_attackers`
team with friendly fire disabled and target the selected human by exact name.
The internal mode name remains `mace-vs-player`. Setup scatters bots around
a ring rather than overlapping their spawn positions. It also changes the
selected human: restores their own camera before survival mode, leaves the bot
team, removes the old `nbmc_duel` supply tag, and teleports them to `0 64 0`.
The player receives an unbreakable netherite spear, Protection IV netherite
helmet/leggings/boots (Feather Falling IV boots), an elytra, and a spare
Protection IV chestplate in hotbar slot 3, plus wind charges, 16 pearls and
64 golden apples. Offhand rockets and 16 reserve inventory stacks total
1,088 rockets. Only specified equipment and supply slots are replaced; the
player's entire inventory is never cleared. Health and food are restored after
equipment is applied. This describes setup behavior, not a new live-combat
validation; the dated historical results below retain their original scope.

This fills empty offhands and 16 empty main-inventory slots with stacks of 64
Flight 1 rockets (up to 1,088 per account). Occupied slots are preserved. It
installs the local arena's `nbmcbot_supply` data pack, which transfers one of
those reserve stacks into an empty offhand and removes the source stack.
Supplies remain finite; the pack does not create rockets. This is a server
helper for the local test arena, not a client inventory feature on arbitrary
servers. It only acts on `nbmc_duel` tagged accounts and does not heal,
resurrect, teleport, or restart their combat tasks. A resupplied session must
not be compared as an unchanged finite-loadout benchmark.

With its flight kit available, the mace controller prioritizes elytra flight
above the opponent, updates its aim using the opponent's current movement and
bounded motion prediction, then equips the chestplate for the downward mace
strike. It equips the elytra again to recover. With a visible target, a complete
flight kit, and rockets, it resumes climbing after at least 12 recovery ticks
and five consecutive observations of a settled glide (vertical speed at least
-0.5 blocks/tick). This avoids waiting several minutes to land after a
high-altitude pass. Rapid falls and missing supplies retain the recovery path;
the normal grounded retry remains available. When
the flight kit is unavailable, it retains the earlier wind-charge and
ender-pearl combination as a fallback. This new flight-and-armor-switch route
is separate from the historical validation below.

If the opponent escapes above a high-altitude dive, the bot completes its
descending leg before retrying, instead of restarting a climb after 12 ticks.
The dive aim stays below its current altitude even after it passes its old
climb height. Near-ground early recovery and the existing dive timeout remain
active, and an actual chestplate swap still needs a valid interception window.

Spear passes use three-dimensional distance for the near-contact commitment
and early exit. Flying directly above or below an opponent no longer counts
as a close pass that triggers an immediate pull-up and turn.

Spear collision avoidance forecasts a short flight segment using the bot's
actual body volume, loaded block collision shapes, current momentum, and
rocket acceleration. Unknown terrain is treated as unsafe. When a predicted
Charge path reaches contact range, prediction includes the following tick's
Exit pull-up, so a legal pass is not rejected solely for continuing its dive.
When the proposed pass is dangerous, it releases the attack charge, suppresses a new rocket,
and steers upward or sideways instead of completing that attack. The same
protection applies while navigating after losing the target. Landing returns
the controller to takeoff; completing a turn requires alignment of the actual
velocity as well as the view direction. `duel_flight_avoidance` records these
interventions. This is bounded collision prediction, not immunity: existing
momentum, knockback, and unobserved world changes can still cause wall or fall
damage; prediction alone does not establish live-server safety.

The 2026-10-05 collision-avoidance candidate passed 167 workspace tests
(including 91 tactics tests), formatting, and strict Clippy. Two separate
45-second vanilla 1.21.11 windows used four bots each: the flat-ground window
recorded two NPC damage events (Spear003 and Spear001), including
`ArenaProbe was speared by Spear001` at 20:15:33; the pit window recorded no
NPC damage. Both recorded zero kinetic damage and zero bot deaths, but each
included two fall-damage events during setup. After the flat target died,
bots lost the target and became idle; this is not 45 seconds of continuous
hits or proof of long-term collision safety. The earlier fresh baseline had
three kinetic and two fall events, one target kill, and no bot deaths; these
limited before/after samples provide no statistical guarantee. Reports are
`.runtime/spear-clearance-after2-flat.json`,
`.runtime/spear-clearance-after2-pit.json`, and
`.runtime/spear-clearance-before-fresh.json`. Candidate executable SHA-256:
`8d42a29adb7ca59ad914093d44d01ec63ecb8d6c289c94064e7c1c9a2d0f5f33`.

The 2026-10-05 recovery/pass regression fix passed 138 workspace tests, strict
Clippy, and formatting checks. Its final 90-second live window recorded two
mace attack attempts and one server-confirmed mace hit: `Spear003 was smashed
by Mace003`. It recorded no runtime or server movement errors. The attacker
returned from recovery to climbing 12 ticks after its attack attempt. This verifies
that the stall is removed and the attack path still works; it does not claim
that every bot hits or that the matchup is balanced. The report is
`.runtime/mace-final-validation.json`; the executable SHA-256 is
`237443101185d48f1359e6136ae54e290f01faac12083567efa919b263e72c16`.

`fight` disables auto-respawn for both groups, then sends `duel mace Spear` and
`duel spear Mace`. Each round uses a single life per account: defeated bots stay
dead instead of respawning with canceled tasks and becoming passive targets.
In the two-team modes, opponent selection uses the opposing account prefix, so the human player is
outside both teams. The scenario has no background round manager: inspect the
result, then run `setup` to enable normal respawning and prepare a fresh round.

## Persistent target lock and search

Both styles lock one opponent by UUID, so a nearer player does not steal the
lock during a pass. A confirmed death, logout, incompatible game mode or
dimension change releases the lock. Reappearing entities are resolved again
by UUID; an old ECS entity handle is never treated as permanent identity.
The command still accepts an enemy-name prefix: `duel mace Spear` selects from
the spear team, while `duel mace Spear001` narrows selection to that name prefix.
An initial `=` requests an exact username: `duel mace =PlayerName` cannot
select `PlayerName2`. Player siege sends this exact target to every active
group and disables bot auto-respawn before fighting.

Losing visibility starts navigation without attacking the remembered point:

- If another living, connected bot on the same team and in the same dimension
  still receives the opponent, its observation supplies the navigation point.
  This uses the team's shared ECS; there is no server-coordinate query or
  cross-process access to the opposing team's local player state.
- With no observer, the bot pursues the last seen point for up to 100 ticks,
  with at most six ticks / 12 blocks of velocity extrapolation. It then searches
  four fixed waypoints 20 blocks around that point, ending at 300 ticks of
  continuous absence (15 seconds at 20 TPS).
- If the search fails, it releases the lock and returns to its position when
  the duel began. There it scans for opponents again. Starting both teams near
  each other makes these home positions useful rally points; arbitrary far-apart
  starting positions do not provide knowledge of unseen opponents.

Airborne search updates direction every tick, turns before boosting and switches
a mace user's chestplate back to elytra when necessary. Ground pursuit uses
bounded pathfinding and invalidates an old route before replacing it. Fresh
local visibility immediately ends pursuit and restores normal combat. Attacks
still require the target in the bot's own entity index and `LoadedBy` set.
Search does not grant attacks through walls or beyond reach.

`duel_tracking` logs `shared`, `last_seen`, `search`, `home`, `idle` and `locked`
transitions plus periodic navigation observations. It records the name, goal,
position and memory age where applicable. `duel_target` records combat target
acquisition. Each bot stores only one target record and one home point.

The 2026-10-05 live 90-second 5v5 validation recorded 50 transitions back to
`locked` after losing local visibility (25 per team), plus actual `shared`
navigation from teammates' observations. Each bot retained the same opponent
throughout its recorded reacquisitions. For example, Mace004 lost Spear004 at
tick 819 and reacquired it at 831. Search timeout and rally behavior are also
covered by regression tests; the live round's brief losses did not reach the
full 300-tick timeout.

All ten connections remained online with no task/runtime errors or server
movement errors. Spear002 killed Mace002; there was no confirmed mace hit in
this window. Persistent lock and reacquisition are demonstrated, not a higher
combat win rate. Combined bot-process RSS peaked at 65,699,840 bytes, excluding
the server. Records: `.runtime/target-lock-5v5-validated.json` and
`.runtime/target-lock-evidence.json`. Runtime binary SHA-256:
`de29aa96d91972baf933379c7ea9daedc77fb31c018c64d451df5361ef504f4b`.

```sh
node scripts/duel-arena.mjs status
node scripts/duel-arena.mjs stop
node scripts/duel-arena.mjs setup
node scripts/duel-arena.mjs fight
```

`status` requests fresh group status and server-reported health for every test
account. `stop` cancels both groups' combat tasks without disconnecting them.
For shutdown, quit both groups and stop the server to save the world:

```sh
node scripts/arena-process.mjs mace-team command quit
node scripts/arena-process.mjs spear-team command quit
node scripts/arena-process.mjs server command stop
```

Logs, FIFO controls and process records are under `.runtime/vanilla-1.21.11`.
Use `node scripts/duel-arena.mjs setup --dry-run` to inspect setup commands
without starting or changing anything. All five actions support `--dry-run`;
in that mode the account count comes from `NBMCBOT_DUEL_TOTAL`, or otherwise
`NBMCBOT_DUEL_COUNT` and its default.
The scripts do not launch the server or bots implicitly.

## Historical 5v5 validation with the earlier kit

On 2026-10-04 at 23:57 PDT, both teams fought in the official local Java
1.21.11 server using the earlier defensive kit: mace bots wore netherite
chestplates throughout the round and carried wind charges and pearls, without
elytras or fireworks. Spear bots used the elytra/spear/firework kit that remains
unchanged. The 60-second sampling window also includes idle time after the
approximately 32-second fight ended. The results and memory figures in this
section apply only to that historical kit and binary, not the new mace flight
and chestplate-switch tactic.

- All ten accounts stayed connected, with no disconnects, task errors, server
  movement validation errors, or server runtime errors during the window.
- The victim connections received 24 server-confirmed opposing-player damage
  events: 21 attributed to spear bots and three to mace bots. These events
  establish hits, not their damage amounts. Four queued mace attacks produced
  three confirmed hits; attempts are not treated as confirmed damage.
- Mace004 smashed Spear005 and Spear002; Mace002 smashed Spear003. The spear
  team won with Spear001 and Spear004 alive. One mace death was attributed to
  a fall caused by Spear004; the other four were spear kills.
- Spear001 recorded seven confirmed hits over repeated charge/exit/turn cycles.
  Its early turn intervals included ticks 319-328, 359-367 and 391-400, showing
  reacquisition instead of the former prolonged orbit.
- The sampled maximum mace height was Y=85.49 above a Y=64 floor. Maximum
  sampled spear speed was 34.05 blocks/second (client velocity observation).

| Shared process | Accounts | Mean RSS (bytes) | Peak sampled RSS (bytes) |
| --- | ---: | ---: | ---: |
| mace-team | 5 | 28,301,088 | 28,409,856 |
| spear-team | 5 | 28,718,949 | 30,457,856 |
| Simultaneous total | 10 | 57,020,037 | 58,867,712 |

Memory excludes the Minecraft server and temporary measurement tools. Each
process was below its 256,000,000-byte limit for this bounded flat-arena round;
this does not establish a limit for larger teams or complex terrain.

The complete record is `.runtime/duel-5v5-validated.json`, including health
queries, damage events, phase transitions, sampled memory and death messages.
The release binary SHA-256 was
`72b88a92046fb8e8c3cf757965a7d119cd30ca3f666961fd0c84a5dd0579f72d`.
Earlier isolated trials and the 1v1 protected-kit trial are retained in
`.runtime/duel-spear-first.json`, `.runtime/duel-1v1-first.json` and
`.runtime/duel-1v1-protected.json`; they should not be merged with this round.

The controllers are deterministic tactical AI, not trained neural networks.
They use current target observations and bounded motion prediction. Projectile
spread, missed catches and missed swings remain possible; a missed pearl/wind
catch can fall back to a wind jump. Each finite round stops combat for a bot
when it dies or runs out of required supplies. `setup` prepares another round.

## Elytra and chestplate mace technique

With the flight kit, `duel mace Spear` keeps the mace selected while boosting
upwards, then aims at the opponent's predicted trajectory. The drop predictor
simulates up to 20 ticks of gravity and horizontal drag after removing the
elytra. A chestplate swap is attempted only for a near interception with enough
time left for equipment and movement updates. Climb height and dive steering
are bounded to avoid an endless upward pursuit of another flying bot. Each
climb selects its height once, including the target's upward velocity and the
initial weapon cooldown, with at most 48 blocks of ascent per climb. The next
pass can climb further to engage a higher opponent. Midair target changes do
not raise the ground reference used for low-altitude recovery. A slow dive
uses another rocket to maintain closing speed; a target that rises above the
attacker causes an early return to climbing.

The swap uses the normal inventory number-key operation. It exchanges the
chest slot and the spare hotbar item, preserving the mace in the main hand and
its attack cooldown. The controller waits for the server's `FallFlying=false`
metadata, then requires a downward fall greater than 1.5 blocks, attack reach,
line of sight and weapon cooldown before attacking. Recovery equips the elytra
again in midair and requests normal gliding at a shallow descent angle, even
when the defeated target was the last visible opponent. This needs no rocket.
The next attack starts after landing. Canceling a task clears any pending
equipment exchange so a fresh setup cannot inherit an old swap.

The mace keeps a live opponent selected throughout the interception. A lost
target during the armored drop causes recovery and landing instead of a sudden
switch to a different opponent. Velocity prediction requires consecutive valid
observations; new targets and stale observations cannot trigger an immediate
predicted drop. Position-packet gaps retain the last measured velocity for at
most three ticks, using elapsed time between actual position changes. Abrupt
turns can still evade a planned hit. Successive horizontal velocity samples
estimate angular velocity, capped at 30 degrees per tick. Steering and drop
prediction extrapolate this turn alongside vertical velocity; stale motion
clears both velocity and turn rate. Unequal packet intervals use the time
between velocity-sample midpoints; the predictor converts the averaged chord
velocity into the next movement step and advances old position samples to the
current tick before computing an interception. This is a short-horizon constant-turn
estimate, not knowledge of an opponent's future control inputs.

`duel_intercept` records the observed target velocity and angular velocity, predicted target point,
time to crossing and horizontal miss. `chestplate_swap` and `armored_drop` are
actions, while `duel_attack` with `technique=elytra_chestplate` is an attempt.
Only the opposing victim's `duel_damage` event confirms server damage.

### Confirmed moving-target smash

The 2026-10-05 01:08:59 PDT 5v5 trial confirmed the complete new technique on
the official 1.21.11 server. Mace001 boosted with its elytra, predicted the
upward, turning Spear003, swapped its chestplate at local tick 445, entered
armored descent at tick 448 and attacked at tick 451 after falling 4.76 blocks.
The prediction used nonzero velocity and a turn rate of -0.343 radians/tick.
Spear003 received a server damage event attributed to Mace001; the server
separately reported `Spear003 was smashed by Mace001`. Connection-local tick
counters have different origins and must not be compared as a shared clock.

The 60-second window recorded two opponent hits (one from each team), all ten
accounts connected, no task/runtime errors and no server movement errors.
Both shared bot processes together peaked at 63,995,904 bytes RSS; individually
they peaked at 31,883,264 and 32,112,640 bytes. This excludes the server and
does not establish a general memory bound for other worlds.

This trial also exposed a recovery defect: Mace001 later died from falling,
as did two other mace bots. The hit validates the interception and chestplate
strike, not safe recovery or a reliable win rate. The subsequent recovery
change is validated separately below.

The full record is `.runtime/elytra-mace-5v5-validated.json`; the compact action
and damage sequence is `.runtime/elytra-mace-validated-evidence.json`.
This trial's binary SHA-256 was
`82ff4d1bcabb26aac1fd10832e56abc2519435d000c62a428d8e4ead23d989d3`.

The recovery build was then tested in a separate 60-second 5v5 round starting
at 01:15:32 PDT. Mace002 attempted an interception, missed, then switched back
to its elytra while descending. Its recovery observation shows
`chest=elytra, gliding=true`; a server query immediately after the window
confirmed `FallFlying=1b`, health 20 and Y=771.24. All ten accounts were alive
at the end, with no death messages, task errors or movement errors. This round
had one confirmed spear hit and no mace hit; it validates recovery separately
from the earlier confirmed smash, not a measured improvement in accuracy.
The two bot processes together peaked at 65,126,400 bytes RSS.

Records: `.runtime/elytra-mace-recovery-validated.json` and
`.runtime/elytra-mace-recovery-evidence.json`. Recovery-build SHA-256:
`4746359889696bf438dd5fcd690d41669932b1b76ece7f8bc71a210493aa880c`.
The final workspace suite passed 117 tests, with strict Clippy and formatting
checks also passing. High-speed direction changes can still defeat the
prediction; neither trial establishes a win rate or guaranteed survival.

## Observing a live round

```sh
node scripts/measure-duel.mjs 60
```

The observer samples both bot processes together every 500 ms, requests fresh
group status every five seconds, and reads test-account health from the server
at the start and end. It never starts or stops processes or changes combat
tasks. The duration defaults to 60 seconds and accepts 1 through 3600 seconds.
Final status and health collection can extend the observation slightly beyond
the requested window. `--dry-run` prints the observation plan without sending
commands or creating a report.

The report is `.runtime/duel-measurement.json`. Bot and server event counts
include only log bytes appended after this measurement began; process-lifetime
log offsets and reported lifetime peak RSS are retained separately. RSS samples
exclude the server and the temporary observer. The simultaneous total is the
sum of both bot processes at each sample.

`duel_action`, `duel_phase`, and `duel_attack` count issued actions, phase
changes, and attack attempts. They do not confirm damage. The separate
`duel_damage` events originate from server damage packets, recorded only by
the victim's connection. The observer preserves those events in the window
and counts `server_confirmed_hits` only when the victim is an account in that
process and the attacker is a recorded account with the opposing team's
prefix. Environmental damage, unknown attackers, same-team hits, and events
from other accounts do not count as opponent hits. These events identify hits
and their source, but provide no damage amount; they do not establish a kill.
Height and speed are
computed from client observations (`velocity` is blocks per tick, multiplied
by 20 for blocks per second), not server-confirmed displacement. The report
keeps initial/final health, server death text, and movement errors separately:
pearls, falls, and regeneration can also change health. `observation_complete`
means the requested sampling window and final checks completed without detected
runtime failures; it does not assert that either combat technique succeeded.

## Initial official-server evidence

Before the defensive-enchantment update, the 1v1 tests used unenchanted armor.
The official server recorded both `Spear001 was smashed by Mace001` and
`Mace001 was speared by Spear001` in separate rounds. These death messages in
`.runtime/vanilla-1.21.11/server-console.log` confirm real attributed kills for
both weapons. They do not by themselves establish repeated successful passes,
long-duration stability, or performance of the later enchanted kit. Final
measurement results must identify the kit and the observation window used.
