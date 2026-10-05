# Shared swarm design

The user approved trying one process with ten accounts and shared chunk data.
Keep the existing single-bot command and add a same-server swarm command. One
Azalea ECS owns the accounts; overlapping chunks reuse the upstream shared Arc.
The CLI accepts one server only, because this Azalea version keys worlds by
dimension and cannot safely share an ECS across unrelated servers.

Each account keeps its own session, plugins, tasks, reconnect state and inventory.
Commands without a target broadcast; `@Username command` targets one account.
Only a global `quit` exits the process. Stopping, disconnecting or failing one
account must not clear another account's queued actions or stop its session.

`status` emits per-account status plus `swarm_status` with the connected count,
process RSS, peak RSS, total loaded chunk references and unique chunk/world Arc
counts. Account statuses label the account. The configured RSS budget applies to
the entire shared process, sampled once; it remains a watchdog, not an OS cap.

Validation uses one multi-connection protocol-774 fixture with consistent world
data and distinct player identities. Integration checks prove actual shared
chunks, targeted stop/disconnect isolation and reconnect. Compare ten separate
processes and one ten-account process against identical fixture chunk counts,
navigation, plugin churn and a 60-second simultaneous online period. Exclude the
server and measurement tools from bot RSS. Record RSS sums honestly: shared OS
pages may be counted repeatedly in the separate-process sum. Do not attribute
all savings to chunks or extrapolate the fixture to arbitrary servers.
