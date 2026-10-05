# NBMCBot

Rust Minecraft Java Edition **1.21.11**（协议 **774**）机器人，基于固定版本 Azalea 0.15.1。当前交付包含命令控制、寻路、原生自动化与受限 Wasm 插件运行时。完整 Meteor/Baritone 功能移植尚未完成，功能状态见 [FEATURES](docs/FEATURES.md)。

现已包含 **网页观战台**：全员实时坐标、血量和战术地图，原版 Minecraft 窗口直播，点击玩家切换观战视角，重开两队对战，以及录像导出 MP4。网页与游戏客户端在本机运行；GitHub 仓库提供源码。启动步骤见 [网页观战与录像](docs/VIEWER.md)。

需要 Rust nightly-2026-03-01。脚本显式选择 rustc、rustdoc 和 Cargo 子命令，避免系统安装的 stable 编译器混入。

```sh
rustup toolchain install nightly-2026-03-01
scripts/cargo.sh test --workspace --locked
scripts/cargo.sh build --release --locked -p nbmcbot --bin nbmcbot
target/release/nbmcbot check
target/release/nbmcbot --config config.example.toml check
target/release/nbmcbot run --server 127.0.0.1:25565 --username NBMCBot --auth offline --no-reconnect
```

`--auth microsoft` 使用 Azalea 的 Microsoft 登录流程。`offline` 是服务端允许离线认证时使用的独立模式。配置文件仅在显式提供 `--config` 时读取；默认值与示例配置一致。认证服务交互与真实 Microsoft 账户登录需要单独验证。

同服多账号可使用一个进程，共享重叠区块的内存数据：

```sh
target/release/nbmcbot swarm --server 127.0.0.1:25565 --usernames Bot01,Bot02,Bot03 --auth offline --no-reconnect
```

`--usernames` 接收逗号分隔的 1–128 个账号，大小写不敏感且不能重复；`all` 是保留名。每组只能连接一个服务器。初始登录间隔 100 毫秒。每个账号有独立的任务、背包、模块和 Wasm 插件实例；同一维度的重叠区块复用内存。此模式要求服务器给各账号发送一致的世界内容，不适用于同名维度下为不同账号显示不同方块的个性化世界。

群组模式下，无前缀的命令或 `@all command` 广播到所有账号，`@Bot02 command` 只操作指定账号。`quit` 和 stdin EOF 退出整个进程；单账号离线用 `@Bot02 disconnect`，恢复用 `@Bot02 reconnect`。账号事件包含 `bot` 字段；`status` 额外输出 `swarm_status`，包含进程 RSS、在线账号数、区块引用总数和实际唯一的区块/世界数量。默认 **256 MB 预算约束整个群组进程**，各账号状态中的 RSS 也表示同一个进程，不应重复相加。

```text
status
@Bot01 goto 7 64 4
@Bot02 stop
@Bot02 disconnect
@Bot02 reconnect
@all plugin load examples/plugins/anti-idle.wat
quit
```

运行时从 stdin 接收以下命令；stdout 输出 JSON 事件，适合脚本消费。**stdin EOF 会主动发送 quit**，因此交互会话或自动化驱动需要保持 stdin 打开。单行最多 4096 字节，超长行丢弃后继续接收下一行；Ctrl-C 请求退出。

```text
status
say hello
goto 7 64 4
follow PlayerName
mine 7 63 4
look 90 0
attack PlayerName
fight PlayerName
fight nearest
duel mace Spear
duel spear Mace
inventory
stop
modules
module anti-idle on
plugins
plugin load examples/plugins/anti-idle.wat
plugin unload anti-idle
disconnect
reconnect
quit
```

使用 `modules` 查询实际模块名。`status` 包含连接、位置、任务、health、food 与 RSS。`goto` 完成事件要求客户端位置到达目标方块；`mine` 等待服务端 `BlockUpdate` 或 `SectionBlocksUpdate` 确认目标变为原方块对应的流体剩余状态后，发出 `task_finished`（task 为 `mine`）。

`fight PlayerName` 持续追赶指定玩家，`fight nearest` 等待并追打附近符合条件的玩家；均自动选空热栏，进入攻击距离且视线通畅、攻击冷却结束后空手攻击。热栏没有空位时返回错误，不会丢弃物品。`stop` 取消追赶和待发攻击。已在官方 1.21.11 本地服务器验证追击、实际扣血与击杀；地址及后台控制见 [本地测试服](docs/ARENA.md)。

`duel mace <敌方名前缀>` 使用珍珠与风弹升空、跟踪目标并下落重锤攻击；`duel spear <敌方名前缀>` 使用鞘翅、烟花和长矛蓄力执行往返冲刺。它们是基于实时状态的战术控制器，不是训练后的神经网络。需要对应装备与有限消耗品，死亡或 `stop` 会结束战斗任务。两队场景、启动和验证方法见 [空中 PvP](docs/DUEL.md)。

配置中的 `modules` 指定启动时启用的原生模块，默认为空。名称只允许 `anti-idle`、`auto-eat`、`auto-totem`、`auto-respawn`，不能重复；运行中仍可用 `module <name> on|off` 调整。

```toml
modules = ["anti-idle", "auto-eat", "auto-totem", "auto-respawn"]
```

```sh
target/release/nbmcbot plugin-check examples/plugins/anti-idle.wat --ticks 10
```

Wasm 插件需要源代码移植，导入 `nbmcbot` ABI，导出 `memory` 和 `on_tick`；支持 `.wat`/`.wasm`，**不支持直接执行 Java/Meteor JAR**。插件隔离、配额、回调原子提交与 ABI 见 [插件说明](examples/plugins/README.md)。

内存目标是机器人及运行所必需辅助进程合计 256,000,000 字节 RSS。配置与插件配额提供资源约束；RSS 监测不是操作系统硬限制。当前机器人不需要独立辅助进程。本仓库的协议 fixture 是测量用服务端，不计入机器人的必需辅助进程。

Linux 提供 cgroup v2 硬限制启动方式，需要可用的 systemd user manager：

```sh
scripts/run-capped.sh run --server 127.0.0.1:25565 --username NBMCBot --auth offline
```

此脚本设置 `MemoryMax=256000000`、`MemorySwapMax=0`，约束该 scope 内的机器人及子进程。超限会导致进程终止；硬限制不能保证任务在给定内存内成功完成。本轮 macOS 环境仅执行脚本语法检查，Linux 实际限制效果尚未验证。

```sh
scripts/cargo.sh build --release --locked -p nbmcbot --bin nbmcbot --example fixture_server
node scripts/measure.mjs
# Ten independent bot processes, with simultaneous RSS sampling:
node scripts/measure-many.mjs
# Compare ten separate processes and one shared process on the same fixture workload:
node scripts/measure-shared.mjs
```

测量脚本启动 release fixture 的随机回环端口，等待实际 spawn 后发送 status、聊天与 goto，执行 32 次插件加载/卸载，等待实际 `task_finished` 后保持一个插件在线 60 秒再退出。记录 OS 峰值 RSS、100ms RSS 采样、JSON 事件及 fixture 观察；结果写入忽略的 `.runtime/measure.json`。超时、运行错误、后台 panic、缺失成功证据或超过内存目标均返回非零。可通过 `NBMCBOT_SOAK_SECONDS=86400 node scripts/measure.mjs` 执行 24 小时版本；默认测试不等于通过 24 小时测试。此单一平坦 fixture 场景不能证明任意服务器或完整插件集合满足内存目标，证据范围见 [VALIDATION](docs/VALIDATION.md)。

`measure-shared.mjs` 为两种运行方式分别启动配置相同的单个多连接 fixture，默认每个账号加载 25 个平坦区块，十个账号在线保持 60 秒。它验证共享模式实际保存 25 份区块主体、250 个账号引用，并把独立进程 RSS 合计与共享进程 RSS 分别记录在 `.runtime/measure-shared-10.json`。可设置 `NBMCBOT_COUNT`（1–32）、`NBMCBOT_SOAK_SECONDS`（1–3600）和 `NBMCBOT_CHUNK_RADIUS`（0–2）；测量不包括测试服或测量工具。

1.21.11 的固定上游功能清单与逐项状态见 [UPSTREAM](docs/UPSTREAM.md)；依赖补丁见 [UPSTREAM-PATCHES](docs/UPSTREAM-PATCHES.md)。
