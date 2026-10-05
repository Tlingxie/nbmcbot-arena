# 功能状态

范围：Minecraft Java Edition 1.21.11 / protocol 774。以下是当前实现范围，不能视为原版 Meteor、Baritone 或其全部 addon 的功能等价移植。

| 能力 | 当前实现 | 验证边界 |
| --- | --- | --- |
| 离线认证连接 | Azalea 会话、配置与 CLI | 本地协议 fixture 覆盖登录与 spawn；实测记录另见 VALIDATION |
| Microsoft 认证 | 接入上游在线认证能力 | 真实账户登录待验证 |
| 聊天 | stdin say 与插件 chat，发送游戏包 | fixture 可观察实际消息 |
| 状态与物品栏 | 连接、位置、任务、health、food、RSS；物品栏查询 | 状态来自当前客户端；物品栏实服交互待验证 |
| goto | 上游寻路、取消、超时与到达判定 | 平坦 fixture 与复杂真实地形分别验证 |
| follow / attack | 可见玩家目标与客户端动作 | 实服多人场景待验证 |
| fight | 持续追击指定或附近玩家、选空热栏、视线与距离检查、等待攻击冷却、stop 取消 | TCP fixture 验证追击、同步空手、攻击节奏与停止；官方 1.21.11 平坦服验证从 12 格外追击、扣血与击杀，见 [ARENA](ARENA.md)；复杂地形另行验证 |
| duel | 重锤珍珠/风弹升空下砸；长矛鞘翅烟花蓄力、脱离、转向再冲刺；按名字前缀选择敌队 | 两种武器均已有原版服击杀证据，控制器及飞行回归测试通过；这是规则驱动的战术 AI，使用有限装备，见 [DUEL](DUEL.md) |
| mine | 开始挖掘、停止；服务端 BlockUpdate/SectionBlocksUpdate 确认目标变为原方块对应流体剩余状态后完成 | task_finished mine 依据服务端包；实服权限、延迟与特殊方块另行验证 |
| 原生自动化 | anti-idle、auto-eat、auto-totem、auto-respawn；配置 modules 指定启动启用，默认空；运行时可开关 | 策略测试及复活、进食、图腾协议场景通过；四项都是简化实现 |
| 重连 | 有限次数、延迟、显式 disconnect/reconnect/quit | 两次连接与立即手动重连协议场景通过；真实服务器恢复待验证 |
| Wasm 插件 | 真正 wasmi 引擎、快照导入、chat/goto/look 动作 | fuel、增长、队列、坐标、陷阱、生命周期有测试 |
| 插件内存回收 | 每插件独立 Engine；失败加载/unload/clear 释放 | EngineWeak 测试确认引用生命周期；RSS 分配器回收行为单独测量 |
| Java/Meteor JAR | 不支持直接运行 | 必须移植源代码到 Wasm ABI 或原生模块 |
| 完整 Meteor 模块与全部 addon | 未完成 | 需要逐模块实现和功能验收 |
| 完整 Baritone 等价性 | 未完成 | 目前复用 Azalea 寻路，不等同所有 Baritone 能力 |
| 256 MB 内存目标 | 本地 release 场景峰值 33.10 MB；配额、RSS 监测及 Linux cgroup 启动脚本 | 仅证明所测场景；瞬时硬上限和完整负载未验收，Linux 脚本未实测 |
