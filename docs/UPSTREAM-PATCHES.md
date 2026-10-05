# 上游补丁

## Azalea 0.15.1+mc1.21.11

`vendor/azalea` 来自 crates.io 发布的 `azalea` 根 crate，精确版本为
`0.15.1+mc1.21.11`。原来源：

- crate：`https://crates.io/crates/azalea/0.15.1+mc1.21.11`
- repository：`https://github.com/azalea-rs/azalea`
- 发布包 `.cargo_vcs_info.json` 记录 commit
  `675e56fd9d9da084b8f923fdab6feda3ebd19568`，dirty 为 true；因此复现源码以发布包为准，不能声称与该 commit 的工作树完全相同。
- 发布包未包含许可证正文；`vendor/azalea/LICENSE.md` 从该 commit 的仓库根目录原样取得，保留 MIT 许可证及版权声明。

根 Cargo.toml 通过 `[patch.crates-io]` 选择本地 `azalea` 和下面记录的
`azalea-client`、`azalea-entity`；其余 Azalea crate 仍使用原 crates.io 依赖。没有修改本机 Cargo 注册表。

根 crate 补丁范围为两个文件：

1. `src/swarm/mod.rs`：`event_copying_task` 的 Disconnect 分支使用
   `try_query_entity::<Option<&Account>, _>` 查询账户。AppExit 会先清空
   ECS，滞后消费的 Disconnect 事件可能引用已经销毁的实体；原
   `get_component` 通过 `query_self` 查询而 panic。现在遇到销毁实体
   或缺失 Account 时结束复制任务，释放其资源。
2. `src/swarm/builder.rs`：缺少 handler state 的两处日志仅打印 entity，
   不再调用 `username()`。该分支已持有 ECS mutex；username 再次
   查询 ECS 会重入非重入锁，并且实体可能已经销毁。这避免错误日志
路径中的死锁或额外查询异常。

## Azalea client：多账号命令取消

`vendor/azalea-client` 同样来自 crates.io 的 `0.15.1+mc1.21.11` 发布包，
原 registry package checksum 为
`eae3437facd56ea9cb0ae5253b540d1cdb78db681cdfd7afb0d40dcc18dd4cbe`。
VCS 元数据与上述根 crate 相同（包括 dirty 标记）；保留同一上游 MIT
许可证正文。Cargo.lock 记录本地 patch，原 checksum 在此保存用于溯源。

共享 ECS 后，停止一个账号不能清空全局消息队列，也不能取出其他账号的
消息再重新写入：重新写入会分配新消息编号，导致旧动作再次执行。
NBMCBot 原位把目标账号的待处理动作标记为无效实体，保留其他消息及编号。

- `src/plugins/mining.rs`：开始/停止挖掘的 reader 遇到不存在的实体时跳过，
  防止已取消动作触发 `unwrap` panic。
- `src/plugins/interact/mod.rs`：开始使用物品的 reader 先验证实体存在，
  防止为已取消动作插入组件时失败。
- `src/plugins/join.rs`：新增 `SuspendedJoin` 标记；已暂停账号的排队重连
  不再启动连接，同时仍完成其回调并继续处理其他账号。
- `src/plugins/attack.rs`：忽略已取消动作的失效攻击者实体；目标已卸载时
  也移除 `AttackQueued`，避免失效攻击一直残留。无效目标不会消耗冷却。

对应的六个 `cancellation_tests` 位于这四个文件。应用层另有消息编号不重放
和真实 TCP 双账号断开/重连隔离回归。修改没有改变区块编码或共享算法，
区块复用使用固定版本 Azalea 原有的 `Arc<RwLock<Chunk>>`。

复现补丁回归：`sh scripts/test-client-patches.sh`。Cargo 不允许直接测试非
workspace member 依赖的 dev-dependencies；脚本建立临时独立 manifest，
复用当前 lock 和原 vendor 源码，使用相同 nightly，结束后移除临时目录。

调查中的实际 backtrace 指向 `Swarm::event_copying_task` →
`Client::get_component<Account>` → `query_self`，发生在动作成功后的
退出阶段。后台任务 panic 曾伴随 exit code 0；因此端到端测试与测量
也必须检查 stderr，不能仅依赖退出码。

最终 workspace、release 及退出路径回归结果由 [VALIDATION](VALIDATION.md)
记录。此文件说明修改目的，不将源码检查视为运行验证。

## Azalea entity：共享位置更新与断线

`vendor/azalea-entity` 来自相同版本的 crates.io 发布包，原 package SHA-256 为
`462b6ec018ae2e25570e52f0a3a7420f585947879fbf9491727157e109d8191c`，
保留同一上游 MIT 许可证。100 账号官方服务器测试中，一个账号超时断线后，
它仍可暂时保留 ECS 实体但已经移除 `MinecraftEntityId`；另一账号收到其
位置更新时，原 `RelativeEntityUpdate` 在排除本地账号前直接 unwrap ID，
导致整个共享进程 panic。补丁忽略这些已清理实体的滞后更新。

相对更新还需要每个观察账号独立推进接收计数，同一服务器更新只应用一次。
原逻辑在跳过重复包时没有推进接收方计数，初始默认值也会重复应用第一包。
修复采用从零开始的计数，并使新加入的观察账号与共享实体当前计数同步。
对应源码是 `azalea-entity/src/plugin/relative_updates.rs` 与
`azalea-client/src/plugins/packet/game/mod.rs` 的已有实体注册分支。

专门回归通过 `sh scripts/test-entity-patches.sh` 执行；client 补丁测试脚本
也明确使用本地 entity patch。首次失败的 100 账号测量保存在
`.runtime/arena-100-memory-first-failed.json`，不作为持续运行通过的证据。

## Azalea client：百账号网络轮询

100 账号实服中，心跳及目标位置更新曾积压十余秒。真实 TCP 回归证明：
在同一个 Tokio 任务 poll 内耗尽 cooperative budget 后，已经可读的心跳被
原同步 `try_read` 当作暂无数据。`src/plugins/connection.rs` 现在对每个
连接每轮最多处理 256 个包，在此有限范围内绕过外层 cooperative budget，
逐轮轮转连接顺序，并在读取前及每个连接批次后推动写任务。测试同时覆盖
已耗尽预算的可读心跳与繁忙连接不阻塞另一连接。

性能采样还显示大量时间花在最终会被丢弃的本地 bot 位置包上。
`packet/game/mod.rs` 对六类移动/速度包提前检查接收账号的实体索引，
目标是其他本地账号时跳过与原逻辑等价的无效更新。自身及远端玩家保持
正常处理，原始收包事件仍可观察；metadata/effect 不走此快路径。

复现新增回归：

```sh
sh scripts/test-client-patches.sh cancellation_tests
sh scripts/test-client-patches.sh local_movement_fast_path_tests
```

前者含原六项取消测试及两项网络轮询测试，后者覆盖六类包的账号边界。
失败实测分别保存在 `.runtime/arena-100-memory-server-oom.json` 和
`.runtime/arena-100-memory-reader-backlog.json`，不能当作稳定通过。

## Azalea physics：鞘翅滑翔与服务器烟花推进

`vendor/azalea-physics` 来自 crates.io `0.15.1+mc1.21.11`，原 package
SHA-256 为 `14a220101605dc6445d5fa0a23e7465e1c099fcb8883bc8c73668311156ea8d2`。
发布包记录源 revision `675e56fd9d9da084b8f923fdab6feda3ebd19568` 和
`dirty: true`，保留上游 MIT 许可证。根 Cargo patch 指向该副本。

原 `travel.rs` 对鞘翅仅有 TODO，启用 `FallFlying` 后仍按普通步行重力
积分。补丁增加滑翔升力、俯仰换能、水平转向及阻力，并继续使用现有方块/
实体碰撞。client `movement.rs` 的姿态与疾跑条件也读取 `FallFlying`，
避免每帧把鞘翅姿态覆盖为站立。

计算参考本地官方 Java 1.21.11 server 的 `LivingEntity.updateFallFlyingMovement`
（混淆类 `chl`，方法 `q(ftm)`）与 `FireworkRocketEntity.tick`
（`ddy.g()`）的 `javap -c -p` 输出；独立实现 Rust 计算，不分发 Java 反编译件。
server bundle SHA-1 为 `64bb6d763bed0a9f1d632ec347938594144943ed`，
官方 server mappings SHA-1 为 `5621e9253f05fd57872bbe7f8ddf5f9a7d525955`。
映射来自官方版本元数据引用的
<https://piston-data.mojang.com/v1/objects/5621e9253f05fd57872bbe7f8ddf5f9a7d525955/server.txt>。

`crates/nbmcbot/src/aerial.rs` 仅在 ECS 中有服务器创建、同世界、明确附着
当前滑翔账号的 `FireworkRocket` 时应用推进。服务器删除该实体后立即
停止推进；调用烟花道具不会凭空注入速度。道具使用通过排队的原版
`UseItem` 包保留手别、瞄准角和预测序号，释放/取消时清理待发送操作，
死亡账号不会发送排队的使用包。风弹冲量、珍珠传送仍由既有服务器包驱动。

验证命令：

```sh
sh scripts/test-physics-patches.sh
sh scripts/cargo.sh test --offline -p nbmcbot --lib aerial::tests
```

滑翔回归先验证未修复时竖直速度 `-0.0784`，修复后约 `-0.01764` 并在
同 tick 积分；道具与烟花回归覆盖死亡过滤、单次序号、手别/瞄准保留、
账号/世界隔离和烟花删除。实服战斗表现需另外验证，此处不把数学与
包序列测试当作命中证据。自定义重力属性及缓降药水仍受上游物理限制。
