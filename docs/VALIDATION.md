# 验证记录

日期：2026-10-04。最新同服共享模式实测：**10 个账号合计平均 63.54 MB RSS，OS 峰值 63.65 MB**；同负载十个独立进程平均合计 372.45 MB。详细证据见下方“同服共享区块对比”。本文保留此前单 bot 与独立进程的历史测量；所有结果均为小规模协议 fixture 场景，不是完整 Meteor/Baritone 或任意服务器负载的验收结果。

## 初始版本环境与产物

以下产物信息与 37 项测试对应初始版本；当前 release 已由下方共享版本替代。

| 项目 | 结果 |
| --- | --- |
| 主机 | macOS / Darwin 25.5.0 / arm64 |
| 工具链 | nightly-2026-03-01，由 scripts/cargo.sh 显式选择 |
| 游戏协议 | Minecraft Java 1.21.11，protocol 774 |
| 依赖 | Azalea 0.15.1+mc1.21.11（本地根 crate 补丁）、wasmi 2.0.0、Cargo.lock |
| release 产物 | target/release/nbmcbot |
| SHA-256 | e5feeec4800dee988b7f537313472ef4096fa18d4661ba9207a428ebc0897da9 |
| 默认线程配置 | 2 个 Tokio worker；3 个 Bevy task-pool worker；stdin 读取线程 |

## 初始版本已执行检查

`scripts/cargo.sh test --workspace --locked`：**37 项通过，无失败、无忽略**。

- 16 项应用策略/配置测试：四个自动化模块、取消与忙碌中断、进食释放和恢复、边界配置、模块名称与重复检查。
- 2 项 stdin 测试：4096 字节边界、超长后恢复、UTF-8、CRLF、EOF。
- 6 项核心测试：命令边界、并发预算、溢出、租约释放与真实 RSS 读取。
- 11 项 Wasm 测试：真实引擎动作、fuel、内存/表增长、递归、动作洪泛、错误导入、非法坐标、原子陷阱、断线、JAR 拒绝、示例与 Engine 回收。
- 2 项 TCP 端到端测试：使用真实 Azalea 协议编解码，覆盖下面的可观察行为。

| 协议行为 | 证据 |
| --- | --- |
| 登录/出生 | 握手协议 774、离线用户名、出生坐标 [4.5,64,4.5] |
| 聊天 | fixture 实际收到 e2e-hello 与插件发出的 plugin-e2e |
| 导航 | 服务端观察到目标方块 [7,64,4] 内的移动包，客户端输出 task_finished goto |
| 挖掘 | fixture 收到完成破坏包，发出方块更新；客户端仅在收到服务端目标更新后输出完成 |
| 自动复活 | 关闭时不发请求；启用后收到 PerformRespawn，服务端回包后健康恢复 20、位置复原、传送已确认 |
| 自动进食 | 先切到有面包的热栏槽 2，再 UseItem；食物更新后 ReleaseUse，并恢复槽 0 |
| 自动图腾 | source 10 / button 40；预测哈希为菜单槽 10 与 45；最终只有槽 45 有一个图腾，无槽 40 重复显示 |
| 插件生命周期 | 真正 WAT 加载、执行、聊天包与卸载 |
| 立即手动重连 | 同批 disconnect/reconnect；两次握手和登录；第二次客户端视距仍为 2，聊天成功 |
| 退出 | 子进程成功结束，stderr 没有 panic |

`scripts/cargo.sh clippy --workspace --all-targets --locked -- -D warnings` 和 `scripts/cargo.sh fmt --all -- --check` 通过。
release 构建、示例配置 check、WAT plugin-check --ticks 10 通过。sh -n scripts/run-capped.sh、node --check scripts/measure.mjs 通过。

## release 内存实测

原始记录：[measure.json](../.runtime/measure.json)。测量从 2026-10-04T11:14:03.087Z 到 11:15:05.052Z，bot 的 time 计时为 61.52 秒。

| 项目 | 结果 |
| --- | --- |
| OS 全进程峰值 RSS | 33,095,680 bytes |
| 100ms 采样峰值 RSS | 33,013,760 bytes，共 610 次有效采样 |
| 最后 status 的 RSS | 33,013,760 bytes |
| OS 报告 swap 次数 | 0 |
| 插件生命周期 | 32 次快速加载/卸载，随后再加载一个插件保持 60 秒，最终共 33 次加载和 33 次卸载 |
| 导航结果 | task_finished goto，最终位置约 [7.5819,64,4.5]，处于目标方块内 |
| 活跃游戏 tick | 最后 status 为 1213 |
| 退出 | code 0，无后台 panic，无运行错误事件 |
| 本场景 256,000,000 bytes 目标 | 通过 |

此测量使用 release 二进制、离线 MeasureBot、视距 2、单个平坦区块 fixture、一次实际聊天、短距离 goto 和小型 anti-idle Wasm。fixture 是测试用服务端；Node、ps 和 time 是测量工具，不承担机器人功能。机器人本身无必需辅助进程，计入完整 bot 进程 RSS。

macOS /usr/bin/time -l 峰值单位为 bytes；Linux time -v 的 kbytes 乘以 1024。ps rss 按 KiB 转为 bytes。通过判定取 OS 峰值，避免 100ms 采样遗漏瞬时峰值。运行时 status 的 RSS 是最近 250ms 监测结果，因此快速加载循环的连续 status 不能独立证明每次加载的瞬时内存；EngineWeak 测试另外验证卸载及失败加载释放 Engine。

## 复现

```sh
scripts/cargo.sh test --workspace --locked
scripts/cargo.sh clippy --workspace --all-targets --locked -- -D warnings
scripts/cargo.sh build --release --locked -p nbmcbot --bin nbmcbot --example fixture_server
node scripts/measure.mjs
```

测量需要 Node.js、/usr/bin/time、pgrep、ps，不需要 npm 包。默认插件在线保持 60 秒；总超时为保持时长加 120 秒。`NBMCBOT_SOAK_SECONDS=86400 node scripts/measure.mjs` 可运行 24 小时场景，**本次没有执行该场景**。

## 10 个 bot 并发实测

2026-10-04 04:35:39–04:36:39 PDT（America/Los_Angeles），同一台主机、同一 SHA-256 的 release 二进制。原始记录：[measure-10.json](../.runtime/measure-10.json)。

每个 bot 是独立进程，连接一个配置相同的本地协议 fixture。全部登录、聊天、goto 到达、32 次插件加载卸载完成后，再统一保持一个插件在线 60 秒。最终十个 bot 均处于连接状态，tick 为 1213，并以 code 0 退出，无错误或 panic。

| 指标 | bytes | 十进制 MB |
| --- | ---: | ---: |
| 十进程同时采样的合计 RSS 峰值 | 330,645,504 | 330.65 |
| 保持阶段平均合计 RSS | 330,615,590 | 330.62 |
| 十个进程各自 OS 历史峰值之和 | 331,481,088 | 331.48 |
| 单进程 OS 峰值最小值 | 32,800,768 | 32.80 |
| 单进程 OS 峰值最大值 | 33,325,056 | 33.33 |

总计 604 次采样，其中 603 次包含全部十个已在线的 bot，597 次属于保持阶段。每次使用一次 ps 调用读取所有已验证 PID 的 RSS，再求和；采样间隔 100ms。合计采样峰值可能遗漏更短的尖峰。各进程 OS 历史峰值之和是另一指标，不能当成精确的同时峰值。

这里相加的是进程 RSS，共享页面可能重复计算，因此不等于新增物理内存或整台机器占用。测试服务端与 Node/time/ps 测量工具不计入 bot 内存。十个 bot 各自均低于 256 MB；它们的合计 RSS 超过 256 MB。此场景没有模拟同一完整服务器上互相可见的十名玩家，也未扩大到真实复杂地图。

复现：`node scripts/measure-many.mjs`。默认十个 bot、60 秒保持；可通过 NBMCBOT_COUNT 和 NBMCBOT_SOAK_SECONDS 调整。此次没有修改机器人程序，仅新增并发测量脚本。

## 同服共享区块对比

2026-10-04 05:38:23–05:40:27 PDT（America/Los_Angeles）。相同 macOS arm64
主机，最新 release SHA-256：
`58bdcad46fdd789e9b8a7cb36771f5648d87a942d0c022fed2126c8e56849f07`。
原始记录：[measure-shared-10.json](../.runtime/measure-shared-10.json)。

两种方式分别连接一个配置相同的多连接 fixture：十个账号、Minecraft
1.21.11/protocol 774、视距 2、每账号 25 个平坦区块。先等待每个账号的
初始坐标、健康状态和完整区块数量，再执行聊天、goto、32 次插件加载/卸载，
随后每账号保留一个 anti-idle Wasm 插件在线 60 秒。此处负载比此前单区块
测试更大，因此用本轮相同负载的 372.45 MB 作为比较基准。

| 指标 | 10 个独立进程 | 1 个共享进程 / 10 账号 |
| --- | ---: | ---: |
| 稳定阶段平均 RSS | 372,446,760 bytes（372.45 MB） | 63,537,152 bytes（63.54 MB） |
| 所有账号在线时采样 RSS 峰值 | 372,473,856 bytes（372.47 MB） | 63,537,152 bytes（63.54 MB） |
| 各进程 OS 历史峰值之和 | 373,637,120 bytes（373.64 MB） | 63,651,840 bytes（63.65 MB） |
| 有效稳定阶段采样数 | 595 | 594 |
| 每账号实际加载区块数 | 25 | 25 |
| 共享进程内区块引用 / 唯一区块主体 | 不跨进程共享 | 250 / 25 |

平均 RSS 降低 **82.94%**。共享模式的整个进程峰值低于 256,000,000 bytes。
这里包含一个运行时承载多个账号带来的收益，不能把全部差值归因于区块共享。
独立进程的 RSS 相加会重复计算共享 OS 页面，因而也不能把这个差值当成
独占物理内存的节省量。服务端与 Node/time/ps 测量工具均未计入 bot RSS。

共享证据来自真实 `Arc` 身份去重：在线保持前后均为 10 个连接、1 个世界、
250 个区块引用、25 个不同区块主体；每个账号自己的 `loaded_chunks` 都为
25。每种方式服务端均观察到十次协议握手和十次初始传送确认。全部账号
实际到达 `[7,64,4]` 目标方块，完成 33 次插件加载和 33 次卸载。共享账号
最后 tick 均为 1235。所有测量进程正常退出，未出现运行错误或后台 panic，
且通过 PID 检查确认没有残留 bot 进程。

最新代码验证：

- `test --workspace --locked`：**43 项通过**，包含双账号同服共享、断开隔离、
  另一账号继续导航与插件聊天、重连后恢复共享，以及原单账号协议回归。
- `sh scripts/test-client-patches.sh`：**4 项通过**，验证已取消实体的挖掘、
  物品使用和排队重连不影响其他账号。
- workspace/all-targets Clippy `-D warnings`、fmt check、release 构建和
  测量脚本语法检查通过。
- 2 账号 / 每种方式 2 秒短测通过，记录在
  [measure-shared-2.json](../.runtime/measure-shared-2.json)。

首次十账号试验因测量脚本过早发送导航而失败，记录保存在
[measure-shared-10-attempt1.json](../.runtime/measure-shared-10-attempt1.json)，
未计入上表。Azalea 的 Spawn 可先于初始传送；仅等待 Spawn 和区块数量
不足。脚本增加初始坐标与 health 就绪检查后，才得到本节的完整成功结果。

复现：

```sh
sh scripts/cargo.sh test --workspace --locked
sh scripts/test-client-patches.sh
sh scripts/cargo.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo.sh build --release --locked -p nbmcbot --bin nbmcbot --example fixture_server
node scripts/measure-shared.mjs
```

多账号命令为 `swarm --server ADDRESS --usernames Bot01,Bot02,...`。
`@Bot01 command` 定向控制，无前缀广播，`quit` 退出整个进程。单组只允许
连接一个服务器，并要求同名维度向各账号提供一致的世界内容；本轮没有
实现跨服务器共享或个性化区块覆盖。测试服没有完整模拟玩家之间的实体
广播、碰撞、复杂地图或长时间运行。

## 未验证与未实现范围

- 真实 Microsoft 账户、真实服务器、复杂地形、密集实体、跨维度、长路径、大型工作负载及 24 小时稳定性没有实测。
- fixture 用于验证协议动作，并非完整 Minecraft 服务端，不能代替上述场景；跟随玩家和攻击玩家也尚无端到端场景。
- 还没有真实第三方 Meteor addon 移植。Wasm ABI 不是现成 Meteor JAR 的兼容层。
- 四个自动化模块均为简化实现。完整 Meteor/Baritone、HUD/渲染、建造/原理图等功能尚未完成，见 [UPSTREAM](UPSTREAM.md)。
- 当前主要使用 Azalea 世界结构与有限寻路节点预算，尚未实现设计中的统一堆分配预算、世界缓存淘汰及全面资源准入。RSS 轮询无法保证瞬时硬上限。
- Linux scripts/run-capped.sh 要求 cgroup v2 和 systemd user manager，设置 MemoryMax=256000000、MemorySwapMax=0。本轮 macOS 只验证脚本语法，没有验证实际 Linux 执行。超限终止是隔离机制，不是成功完成工作负载的证据。

因此，本次交付是通过所列测试的可运行内核，原始“完整功能且总 RSS 始终不超过 256 MB”的目标仍未完成。
