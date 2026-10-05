# 网页观战与录像

观战台使用真实 Minecraft Java 1.21.11 客户端的窗口画面。
它提供全员坐标、血量、队伍、死亡/离线状态与二维地图，通过服务器
`spectate` 指令切换所选观察者的游戏相机。一个窗口同时显示一个视角；
点选目标只改变名单选择，点击“跟拍此人”才切换游戏视角。

## 环境

- Rust nightly-2026-03-01、Java 21、Node.js 22 或更新版本。
- Chrome 等支持 `getDisplayMedia`、Canvas captureStream 和 MediaRecorder 的桌面浏览器。
- PATH 中的 FFmpeg，用于将录像转为 H.264 MP4。
- 本机运行 Minecraft 1.21.11 客户端，登录竞技场。
- 现有后台控制脚本使用 POSIX FIFO，支持 macOS/Linux；Windows 需要适配。

网页没有 npm 第三方依赖，执行 Node 脚本即可。

## 从源码启动

```sh
rustup toolchain install nightly-2026-03-01
chmod +x scripts/*.sh
sh scripts/cargo.sh build --release --locked -p nbmcbot --bin nbmcbot
mkdir -p .runtime/vanilla-1.21.11
cp examples/arena-server.properties .runtime/vanilla-1.21.11/server.properties
```

从 Mojang 官方版本元数据下载 1.21.11 的 `server.jar`，放入该运行目录。
相关官方链接和校验值见 [ARENA](ARENA.md)。阅读 Minecraft EULA 后，由你自行
决定是否接受并在运行目录配置 `eula.txt`。仓库不分发 Minecraft 的程序或素材。

```sh
node scripts/arena-process.mjs server start
NBMCBOT_TELEMETRY_ADDR=127.0.0.1:4211 node scripts/arena-process.mjs mace-team start
NBMCBOT_TELEMETRY_ADDR=127.0.0.1:4211 node scripts/arena-process.mjs spear-team start
node scripts/viewer-process.mjs start
```

首次启动服务器并生成世界后，在控制台执行以下测试场地规则；站立高度为 64：

```sh
node scripts/arena-process.mjs server command gamerule minecraft:keep_inventory true
node scripts/arena-process.mjs server command gamerule minecraft:spawn_mobs false
node scripts/arena-process.mjs server command gamerule minecraft:max_entity_cramming 0
node scripts/arena-process.mjs server command gamerule minecraft:advance_time false
node scripts/arena-process.mjs server command gamerule minecraft:advance_weather false
node scripts/arena-process.mjs server command time set day
node scripts/arena-process.mjs server command weather clear
```

客户端连接 `127.0.0.1:25566`，浏览器打开 **http://127.0.0.1:4210**。
服务器与网页均仅绑定本机。示例离线认证适用于本地测试。

1. 在网页前台点击“连接 Minecraft 窗口”，在浏览器选择器中选择游戏窗口。
2. 在 Minecraft 按 F3+P 关闭失去焦点时暂停，再按 Esc 退出菜单。保持客户端运行。
3. 在网页选择实际在线的观察者账号；选择 bot，再点击“跟拍此人”。
   观察者会变为旁观模式。可用“自由视角”退出目标绑定，仍保持旁观模式。
4. 选择“重锤 vs 重锤”或“重锤 vs 长矛”，点击“开启新局”。
   此操作复活、装备并重置两队位置；每人最多配备 1,088 个烟花，倒计时后开战。
5. 点击“开始录制”，结束时点击“停止并导出”，等待“下载 MP4”。

若客户端正在菜单中，录像也会如实录到菜单。选择的共享窗口必须属于观察者账号；
网页无法识别其他 Minecraft 窗口的登录身份。浏览器必须保持前台可见，避免后台
节流降低画面帧率。输出画布目标为 1280×720、30 FPS，实际帧率取决于游戏及浏览器。
当前录制为无声视频，不采集麦克风或系统声音。

## 数据和资源边界

- bot 在启动时读取 `NBMCBOT_TELEMETRY_ADDR`；已启动的进程必须重启才能启用。
  独立 10Hz 采样涵盖待机、死亡和断线，不依赖攻击事件。未启用时没有发送线程。
- 服务器控制台每约 250ms 补充真人位置/朝向/血量，每 3 秒更新在线名单与维度。
  超过 2 秒的位置显示过期，超过 5 秒没有观测的连接显示离线。
- 网络发送、序列化与游戏锁分离；仅 loopback UDP，容量 1 的有界队列，忙时丢帧。
  网页、原版客户端和 FFmpeg 是额外观战成本，不包含在 bot 的内存测量中。
- 浏览器每约 1 秒上传录制片段。单段最多 16MiB，单录像最多 2GiB，
  最多一个录制或转码任务。文件存放于 `.runtime/recordings/<id>.mp4`，
  原始录制和元数据使用同一个 ID。死亡后客户端物理位置不再用于地图，以免显示漂移坐标。
- MP4 转码失败时保留原始 WebM/MP4，并按真实格式提供下载。页面关闭前应停止录制。
  服务重启后中断录像会保留已有内容并标记失败；历史文件需自行清理。
- 本机 HTTP 写接口要求同源与随机会话 token，不开放任意控制台命令。
- `NBMCBOT_VIEWER_PORT` 和 `NBMCBOT_TELEMETRY_PORT` 可调整监听端口；相应调整 bot 的 UDP 地址。

## 验证与关闭

```sh
npm test --prefix web
sh scripts/cargo.sh test --workspace --locked
node scripts/viewer-process.mjs status
node scripts/viewer-process.mjs stop
node scripts/arena-process.mjs mace-team command quit
node scripts/arena-process.mjs spear-team command quit
node scripts/arena-process.mjs server command stop
```

关闭网页服务不会停止游戏服务器或机器人。录像、世界、账号缓存、日志、
本地配置和构建产物均在 Git 忽略范围内，不上传到公开仓库。
