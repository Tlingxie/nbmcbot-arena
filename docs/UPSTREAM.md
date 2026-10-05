# Minecraft 1.21.11 上游功能基线

采集日期：2026-10-04。机器可读清单：[upstream-inventory.json](upstream-inventory.json)。

这份清单确认上游源码中存在的功能，不证明 NBMCBot 已移植或达到行为等价。**所有条目的 `parity_status` 均为 `unverified`**；目前没有任何条目被这份清单认定为完整 parity。NBMCBot 的局部协议测试结果仍单独记录在 [VALIDATION.md](VALIDATION.md)。

## 固定版本

| 项目 | 官方引用 | 固定 commit | Minecraft 版本证据 |
| --- | --- | --- | --- |
| Meteor Client | `refs/heads/1.21.11` | `48030717dc0d4898a195a713fb048bf597fc5b54` | [版本目录第 3 行](https://github.com/MeteorDevelopment/meteor-client/blob/48030717dc0d4898a195a713fb048bf597fc5b54/gradle/libs.versions.toml#L3) 明确为 `1.21.11` |
| Baritone | `refs/tags/v1.17.0` | `23723891da460ef15797b02fe5b385b0c5b163cc` | [gradle.properties](https://github.com/cabaletta/baritone/blob/23723891da460ef15797b02fe5b385b0c5b163cc/gradle.properties#L11) 与 [正式发布页](https://github.com/cabaletta/baritone/releases/tag/v1.17.0) 均明确为 `1.21.11` |

Meteor 官方 refs 中没有找到 `1.21.11` 发布标签，因此固定的是该官方分支在采集时的提交；没有猜测其对应某个 `v0.5.x` 标签。该提交日期为 2026-08-10。Baritone 固定提交日期为 2026-08-31。以后分支移动不自动改变这份基线。

有一处真实版本差异必须保留：Meteor 这个提交的 [版本目录第 14 行](https://github.com/MeteorDevelopment/meteor-client/blob/48030717dc0d4898a195a713fb048bf597fc5b54/gradle/libs.versions.toml#L14) 仍声明 `meteordevelopment:baritone:1.21.10-SNAPSHOT`，对应 Meteor 自己的 Baritone fork。它不是这里选定的官方 `cabaletta/baritone v1.17.0`。因此本清单是两个分别针对 Minecraft 1.21.11 的功能基线，**没有宣称这两个产物已经组成可运行、互相兼容的 Meteor 安装组合**。没有采用 `master` 或任何 `26.x` 分支的功能清单。

## 清单范围与计数

| 范围 | 数量 | 取数依据 |
| --- | ---: | --- |
| Meteor 内置模块 | 170 | [Modules.java 注册表](https://github.com/MeteorDevelopment/meteor-client/blob/48030717dc0d4898a195a713fb048bf597fc5b54/src/main/java/meteordevelopment/meteorclient/systems/modules/Modules.java#L394) 的实际 `add(new ...)` 调用 |
| Meteor 命令对象 | 39 | [Commands.java 注册表](https://github.com/MeteorDevelopment/meteor-client/blob/48030717dc0d4898a195a713fb048bf597fc5b54/src/main/java/meteordevelopment/meteorclient/commands/Commands.java#L32)，包含构造器声明的别名 |
| Baritone 命令对象 | 42 | [DefaultCommands.java](https://github.com/cabaletta/baritone/blob/23723891da460ef15797b02fe5b385b0c5b163cc/src/main/java/baritone/command/defaults/DefaultCommands.java#L30)，包含重定向及四个执行控制命令 |
| Baritone 命令名称，含别名 | 63 | 以上命令构造器中的主名称与别名，不重复计为独立行为 |
| Baritone 顶层 process 实现 | 10 | [process 源码目录](https://github.com/cabaletta/baritone/tree/23723891da460ef15797b02fe5b385b0c5b163cc/src/main/java/baritone/process) 中的 `*Process.java`，包含内部协调 process |

Meteor 分类为 Combat 29、Player 31、Movement 31、Render 37、World 25、Misc 17。`Excavator` 与 `InfinityMiner` 的注册受 `BaritoneUtils.IS_AVAILABLE` 条件控制，JSON 已标注。Baritone 的 `SchematicaCommand` 在注册表中被注释，记录为未注册源码项，不计入 42 个命令。

每条 JSON 记录含稳定 ID、原始类名、主名称、状态及固定 commit 的源码链接。Meteor 模块还记录类别和静态提取的设置名称；命令记录别名。`literal_setting_names` 仅表示源码中直接出现的 `.name("...")` 字面量，不保证覆盖动态设置、取值范围、模式、默认值或相关模块的共享设置。

计数是源码注册条目数，不是独立用户操作或已通过测试的功能数。例如 Meteor 的 `commands` 声明 `help` 别名，同时也注册 `HelpCommand`；JSON 原样保留源码关系，没有据此承诺解析优先级。Baritone 的 process 与命令存在功能重叠，也不能直接相加计算完成率。

## 现有简化实现不等于上游 parity

| NBMCBot 模块 | 对应 Meteor 模块 | 已知仍需核对或实现的行为 |
| --- | --- | --- |
| `auto-eat` | [AutoEat](https://github.com/MeteorDevelopment/meteor-client/blob/48030717dc0d4898a195a713fb048bf597fc5b54/src/main/java/meteordevelopment/meteorclient/systems/modules/player/AutoEat.java) | 当前是固定食物集合、热栏选择及饥饿阈值策略；上游包含食物黑名单、食物优先级、副手与可选主背包搜索、生命/饥饿组合阈值，以及暂停/恢复战斗模块和寻路等行为。 |
| `auto-totem` | [AutoTotem](https://github.com/MeteorDevelopment/meteor-client/blob/48030717dc0d4898a195a713fb048bf597fc5b54/src/main/java/meteordevelopment/meteorclient/systems/modules/combat/AutoTotem.java) | 当前是将图腾放入副手的有限重试策略；尚未覆盖上游模式、延迟、生命与吸收生命、潜在爆炸/坠落伤害判断等完整语义。 |
| `anti-idle` | [AntiAFK](https://github.com/MeteorDevelopment/meteor-client/blob/48030717dc0d4898a195a713fb048bf597fc5b54/src/main/java/meteordevelopment/meteorclient/systems/modules/player/AntiAFK.java) | 当前定期改变朝向；上游还具有多种可配置动作、随机化、消息及其时间控制，名称也不同。 |
| `auto-respawn` | [AutoRespawn](https://github.com/MeteorDevelopment/meteor-client/blob/48030717dc0d4898a195a713fb048bf597fc5b54/src/main/java/meteordevelopment/meteorclient/systems/modules/player/AutoRespawn.java) | 当前根据死亡观测请求重生；上游还保存死亡 waypoint 并处理死亡界面事件，这些交互尚未建立等价性证据。 |

四项的 `nbmcbot_status` 都明确为 `simplified_implementation_not_parity`。其他条目的 `not_mapped_or_verified` 表示尚未逐条完成映射或验证；它不因 NBMCBot 恰有同名命令而自动改变。当前 `goto`、`mine`、`follow` 依赖 Azalea，实现范围也不能替代 Baritone 的完整命令、process、设置与导航语义。

## 尚缺的范围

- 第三方 Meteor addon 的仓库集合、1.21.11 兼容 revision、依赖和许可尚未指定；JSON 明确将 addon 基线标为 `missing`。因此不能声称已经枚举或完成“全部 addon”。
- 本次逐项列出了内置模块与命令，但没有枚举所有命令子树、参数组合、设置语义、HUD、界面、宏/账户/好友等独立系统、Baritone 设置、goal、移动原语或 schematic 格式。完整功能目标仍需进一步分解。
- Render 类的 37 个模块全部保留在目标清单中，未因机器人没有游戏窗口而自动标为完成或不适用。需要逐项定义可观察的等价输出，才能判断是否达到目标。
- 尚未运行固定版本的 Meteor/Baritone 与 NBMCBot 的差分场景，也没有测得“完整功能同时启用”的 RSS。256 MB 目标和功能 parity 必须分别提供场景证据。

## 状态推进规则

将条目从 `unverified` 推进前，至少补齐固定上游 revision、对应 NBMCBot 实现位置、设置/命令契约、可重放场景和结果证据。包含库存或世界修改的行为应区分客户端预测与服务端确认；取消、死亡、重连和异常路径也属于验证范围。不能通过改名、移除未实现模块或把整个类别设为“不适用”来增加完成比例。

## 复核方法

采集使用官方 Git refs 和固定 checkout，源码下载位于被忽略的 `.runtime/upstream/`。没有把上游 Java 实现复制到生产 Rust 文件。提取采用注册表中的活动调用，而不是将所有 Java 文件都当作模块或命令；每项都带源码行链接，便于独立复核。

```sh
git ls-remote --heads --tags https://github.com/MeteorDevelopment/meteor-client.git
git ls-remote --heads --tags https://github.com/cabaletta/baritone.git refs/heads/1.21.11 refs/tags/v1.17.0
jq '.upstreams | map_values(.counts)' docs/upstream-inventory.json
jq '[.upstreams[] | (.modules // [])[], (.commands // [])[], (.processes // [])[]] | group_by(.parity_status) | map({status: .[0].parity_status, count: length})' docs/upstream-inventory.json
```

许可证与已改编 Azalea fixture 的声明见 [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)。实际源码移植时需要保留对应上游许可，项目的 MIT 元数据不能将其他作者的 GPL/LGPL 代码改为 MIT。
