# Wasm addon ABI

Load `anti-idle.wat` with the application's plugin load command. `.wasm`
modules use the same ABI. IDs come from the filename stem: 1-64 ASCII
letters, digits, underscores or hyphens. Duplicate IDs are rejected.

Every module exports `memory` and `on_tick: () -> ()`. Start functions are
rejected. Allowed imports from `nbmcbot`:

| Import | Wasm signature | Behavior |
| --- | --- | --- |
| chat | `(i32 pointer, i32 length) -> ()` | UTF-8 chat from exported memory |
| goto | `(i32 x, i32 y, i32 z) -> ()` | Navigation action; x/z in [-30000000, 30000000], y in [-2048, 2048] |
| look | `(f32 yaw, f32 pitch) -> ()` | Finite angles; pitch in [-90, 90] |
| tick | `() -> i64` | Current snapshot tick, unsigned bit pattern |
| health | `() -> f32` | Current health |
| position | `(i32 axis) -> f64` | Axis 0, 1 or 2 |

Callbacks run only while connected. Actions commit only after the callback
returns successfully. Traps discard that plugin's entire callback queue.
Fuel resets each callback; host action and chat-copy work also consumes fuel.
Errors are visible through `PluginInfo.last_error`; later callbacks can retry.
Each plugin owns a separate wasmi engine. Unload and clear drop guest stores,
translated code and retained queues; failed loads also release their engine.

Default limits permit eight plugins, 256 KiB input per plugin, 1 MiB total
module input, 1 MiB linear memory per plugin and 8 MiB total reserved linear
memory. Each store admits one instance, memory and table. Tables allow 1024
elements, recursion 64 calls, stack 4096 values, fuel 100000 per callback,
16 actions and 256 bytes per chat. Aggregate linear admission reserves the
entire configured per-plugin allowance even when initial memory is smaller.
wasmi's strict module translation limits are enabled. Input budgets are
admission accounting, not exact compiled heap size or a process RSS cap.

This is an ABI for source-level addon ports. It does not execute Java or
unmodified Meteor JARs.
