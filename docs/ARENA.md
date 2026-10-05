# Local fist arena

Minecraft Java Edition 1.21.11 test server, bound to this computer only:
`127.0.0.1:25566`. The pre-existing listener on port 25565 was left untouched.

The official Mojang server is stored under `.runtime/vanilla-1.21.11` (ignored
runtime data). Its download URL is recorded in `version.json` and its verified
SHA-1 is `64bb6d763bed0a9f1d632ec347938594144943ed`. It requires Java 21.
Source: [official version metadata](https://piston-meta.mojang.com/v1/packages/4f6bd9388f12e9d7adc2ded64acba66212d60521/1.21.11.json).

The local arena uses offline authentication, survival mode, PvP enabled, a flat
world at foot height 64 and spawn `[0,64,0]`. Day/weather are fixed, mob spawning
is disabled and inventory is retained on death. RCON/query are disabled. Server
memory is separate from the bot process memory budget.

The arena now allows 128 players with view/simulation distance 2. Entity
cramming damage is disabled so simultaneous bot spawning does not crush the
accounts before they can spread out. `allow-flight=true` prevents the vanilla
floating check from kicking players during the crowded knockback test; it does
not grant the bots a flight movement mode.

Start the server from its directory with Java 21:

```sh
java -Xms512M -Xmx2G -XX:MaxDirectMemorySize=2G -XX:+UseG1GC -jar server.jar nogui
```

Start the bot from the repository root:

```sh
target/release/nbmcbot run --server 127.0.0.1:25566 --username FistBot --auth offline --no-reconnect
```

After the bot joins, send these commands to its stdin:

```text
fight nearest
status
stop
```

`fight PlayerName` follows one named player; `fight nearest` waits for an eligible
visible player and chooses a nearby target. `stop` cancels pursuit and pending
attacks. `quit` exits the bot. The server console accepts `stop` to save and shut
down the server.

## Background control

The current arena runs as detached processes with open FIFO stdin. The control
script exits after each invocation, so it does not add a resident Node process
to the bot memory budget. Run commands from the repository root:

```sh
node scripts/arena-process.mjs server status
node scripts/arena-process.mjs bot status
node scripts/arena-process.mjs bot command fight nearest
node scripts/arena-process.mjs bot command stop
```

If stopped, start the existing configured arena with `server start`, followed by
`bot start` and `bot command fight nearest`. To shut down deliberately, use
`bot command quit` and `server command stop`. The helper expects the downloaded
server JAR, configured runtime directory, release bot binary and Java 21 to
already exist. Logs and process records are under `.runtime/vanilla-1.21.11`.

The current 100-account deployment uses two shared processes, each with its own
256,000,000-byte RSS budget. `swarm-a start` starts `Fist001` through `Fist050`;
`swarm-b start` starts `Fist051` through `Fist100`. Each group staggers initial
logins by 100 ms. After they join, arm both groups:

```sh
node scripts/arena-process.mjs swarm-a command fight PlayerName
node scripts/arena-process.mjs swarm-b command fight PlayerName
```

Use `swarm-a command stop` and `swarm-b command stop` to stop pursuit, or
`command quit` to disconnect the corresponding group. A command such as
`swarm-a command @Fist001 status` targets one account. Chunks are shared within
each process; overlapping chunks can have one copy per group. The two groups'
RSS must be added when reporting total bot memory, not compared individually
and then described as a 256 MB total.

The experimental `swarm start` launches all 100 in one process. In live combat
it reached 258,621,440 bytes and exited at the configured 256 MB budget; the
failed record is `.runtime/arena-100-memory-single-cap.json`. Account-count
support does not guarantee that every workload fits a given memory budget.
The swarm uses the existing bounded reconnect policy. A reconnect or death
cancels the affected bot's task; broadcast `fight PlayerName` again to rearm it.
Set `NBMCBOT_TRACE_KEEPALIVE=1` when starting the swarm to log received heartbeat
IDs and observation timestamps while diagnosing disconnects; those events do
not prove that a reply reached the server.

`node scripts/measure-arena.mjs 60` samples the running swarm for 60 seconds,
without stopping it, and saves `.runtime/arena-100-memory.json`. Its report
excludes the Minecraft server and the temporary measurement tools. Shared
chunk counts and account count come from the bot's `swarm_status` events.
For the two-group deployment, use `node scripts/measure-arena-split.mjs 120`.

## Official-server validation

On 2026-10-04, FistBot started at `[100.5,64,0.5]` and the unarmed test target
ArenaProbe started at `[112.5,64,0.5]`. The target's server-reported `Health`
was initially `20.0f`. After `fight ArenaProbe`, FistBot pursued the target,
continued chasing after knockback and dealt real damage:

- 06:03:16 PDT: server-reported health `11.333332f`.
- 06:03:28 PDT: `ArenaProbe was slain by FistBot` in the server log.
- 06:03:44 PDT: server-reported health `0.0f`.

The bot inventory was empty. Its `combat_attack` events recorded
`empty_hand: true` and `cooldown_remaining_ticks: 0`. Observed bot peak RSS
during this short run was 38,928,384 bytes; this is not a long-duration or
complex-terrain benchmark. The server process is measured separately.

The TCP combat regression also covers waiting for a target, selecting an empty
slot before attacking, attack/swing packets, cooldown spacing, chasing a target
that moves away and no further attacks after `stop`. The real-server result
above supplies the damage verification that a protocol fixture cannot provide.

The initial combat source passed 49 workspace tests, six focused vendored-client
cancellation tests, formatting and Clippy with warnings denied. A final pursuit
radius correction additionally tests that an off-center target at integer foot
height has a reachable path goal; complex terrain remains outside this arena
validation.

Moving-target pursuit also cancels the old route and in-flight path calculation
before requesting a new route. Azalea normally retains part of the previous
route and appends the new path; that behavior caused bots to visit the player's
old position after a turn. Redirection remains limited to once per ten ticks
when the target changes blocks, with per-account cancellation isolation.

The 100-account investigation found a separate network delay: exhausted Tokio
cooperative I/O budget made readable sockets appear empty during the shared
ECS update. Bounded reads with a rotating connection order now avoid that false
empty result. Movement updates that would be discarded for another local bot
also skip their expensive handler setup. The workspace has 52 passing tests;
focused vendor checks cover eight cancellation/network cases, two local-move
cases and four shared-entity cases. Live measurement results below must still
be interpreted separately from these regressions.

On 2026-10-04, the two-group deployment completed a 120.303-second sample
from 13:50:11.780 through 13:52:12.083 UTC. Both groups remained at 50 connected
accounts, with zero disconnects or runtime errors. The player was dead and
the armed bots were waiting for respawn; neither process recorded an attack.
This is an idle/waiting measurement, not a sustained combat benchmark.

| Process | Accounts | Mean RSS (bytes) | Sampled peak RSS (bytes) |
| --- | ---: | ---: | ---: |
| swarm-a | 50 | 98,353,152 | 98,353,152 |
| swarm-b | 50 | 100,836,112 | 100,958,208 |
| Simultaneous total | 100 | 199,189,264 | 199,311,360 |

The total peak is 199.31 decimal MB, excluding the Minecraft server and
temporary measurement tools. Each bot process stayed below its 256 MB limit
for this workload. The complete samples and status events are stored in
`.runtime/arena-100-split-memory.json`. Both processes used release binary
SHA-256 `46ab91c10e2390d8a80e138a882d3ec77bcb4e1d67b35f52eabfa749d38a7b02`.
