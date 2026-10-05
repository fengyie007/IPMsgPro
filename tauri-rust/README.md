# 迅秋 Rust 预览版（Rust + Tauri 2）

这是旧 C++/TauriCPP 项目旁边的**独立重写试验**，目前包含核心文本通信及第二阶段2A图片接收，不是完整功能迁移。

- 原 C++ 源码、构建脚本、数据库及注册表设置不修改、不共享。
- React/TypeScript/Tailwind/Zustand 界面从原工程复制后独立适配；后端是 Rust，没有调用旧 exe/DLL 代替实现。
- Rust 协议、UDP、SQLite、配置逻辑位于 `crates/ipmsg-core`，不依赖 Tauri，可独立测试。
- Tauri 2 壳位于 `src-tauri`，负责窗口、托盘、平台目录、网卡枚举及 IPC。

## 当前状态

**已通过离线检查：**

- Rust核心当前68个单元测试通过；5个图片接收/存储集成测试及8个原有UDP集成测试通过，包含兼容公告、提交拒收竞态和未提交资产清理。
- `tools/test-store.cjs`：9组前端消息store逻辑测试通过，包括图片元数据、历史、去重及清空（IPC使用测试替身）。
- 两个真实飞秋LZW样本用Rust解码后，与发送端BMP原图所有像素一致（200×268及394×198）。
- Rust核心和Tauri桌面`cargo check --offline --locked`已通过；`npm run check`完整前端类型检查已通过（修复异步命令返回类型后）。

**实机状态：**

用户已补齐依赖并成功构建、启动GUI。首次局域网回归发现两个问题：同名不同端点会覆盖，以及Windows UDP探测未监听端口触发10054后接收循环退出；代码与对应回归测试已修正。最新实机日志已确认飞秋发现、发送回执与收信事件恢复，并且10054之后仍持续接收；完整UI自动化和安装包验收尚未执行。

## 功能范围

实现：

- IPv4 UDP 用户发现、直接添加 `IP:port`、上线/离开/离线状态；已知用户每60秒探测，连续三轮无响应才标记离线。
- 按对端声明的组名展示通讯录、搜索、本人组置顶。
- GBK/UTF-8 文本收发、多行、现有文本表情格式、飞秋字体尾标清理。
- 消息回执、最多三次发送尝试、稳定包号重试、错误来源回执拒绝、接收去重与超时失败。
- SQLite 历史、分页、搜索、清空、最近会话；重启遗留的发送中记录标记失败。
- Rust JSON 配置持久化、错误提示、默认关闭到托盘、托盘显示/退出。
- 非前台收到消息时通过 Tauri 请求任务栏注意（非完整系统通知或提示音迁移）。

暂不支持：图片发送、截图、TCP文件传输、文件拖放、IP范围扫描、自定义网段、提示音、旧版数据导入。发送图片/截图/文件按钮仍禁用；普通文件不会自动接受。2A新增接收飞秋内嵌图片、缩略图/应用内原图查看和图片历史，真实GUI收图仍待用户验收。详见 [MIGRATION.md](MIGRATION.md)。

### 2A 图片接收

- 支持经过长度、CRC和像素边界验证的飞秋LZW DIB，以及经过解码验证的原始PNG/JPEG；统一保存为PNG。
- 分片按端点联系人ID和wire图片ID隔离；最终片在解码、写盘和数据库提交成功后才ACK。重复引用/分片不会重复创建图片消息，不等待引用才能ACK。
- 图片资产位于本实例 `images/<assetId>/full.png` 与 `thumb.png`。历史保存元数据，不存整图base64；SQLite schema升级为2，旧文本历史保留。
- 前端通过 `image.read {assetId,thumbnail}` 获得受控 `ipmsg-image` 图片URL，不能传任意磁盘路径。只有主窗口的本地页面可以访问该协议。
- 发送能力与接收能力分开：`imageReceive=true`，`imageSend=false`，汇总`images=false`。截图发送属于后续2B/2C。
- 线上报文使用与原C++版一致的飞秋兼容扩展头，避免被飞秋当作不支持图片的普通IPMsg；应用自身版本不变，未实现的文件/加密功能不因扩展头而启用。若飞秋仍提示“对方使用飞鸽”，请重启新版Rust并在飞秋刷新联系人/重新打开对应会话，避免旧客户端类型缓存。`--verbose`日志可看到 `Discovery wire version=1_lbt6_0...`。
- 单载荷16MiB、DIB64MiB、1600万像素、4个活动拼接/32MiB压缩预算；后台解码串行执行。坏图、冲突或超时不会作为成功图片展示。
- 清空历史暂不删除图片文件，避免误删仍被引用的资产；不提供自动图片GC。异常退出可能留下未引用资产，后续将增加可审查的清理流程。

## 环境与依赖

- Windows x64，Visual Studio C++ Build Tools、WebView2 Runtime。
- Rust stable MSVC（本次核心测试环境：Rust 1.98.1）。
- Node.js 18+，npm。
- Tauri 2（当前声明 2.11.5），bundled SQLite，不要求另装 SQLite DLL。

第一次准备依赖通常需要联网，以下命令**由使用者按自己的网络策略执行**；自动实现过程没有联网下载：

```powershell
cd tauri-rust
npm install
cargo fetch --manifest-path src-tauri/Cargo.toml
```

依赖不足时按 Cargo/npm 实际错误补齐，不伪造锁文件。

为使核心离线测试不被缺失的 WebView 依赖阻塞，根 workspace 只包含 `ipmsg-core`，`src-tauri` 是独立 Cargo package：

- 根 `Cargo.lock` 已由实际离线解析生成。
- `src-tauri/Cargo.lock`、`package-lock.json` 由对应依赖首次成功解析后生成，应用项目应将生成后的锁文件纳入版本管理。

## 检查、开发与构建

```powershell
# 不启动GUI的核心检查（在 tauri-rust/ 内）
cargo test --offline -p ipmsg-core
cargo check --offline -p ipmsg-core
cargo fmt --all -- --check

# 桌面检查（依赖准备后）
cargo check --offline --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
npm run check
npm run test:store

# 启动开发版；Vite使用127.0.0.1:1420
npm run tauri -- dev

# 编译前端+桌面应用，并生成NSIS安装包
npm run tauri -- build
```

打包可能还需要下载打包工具；这与 Rust 核心离线检查不同。

如果希望明确指定运行参数，可使用两个终端：

```powershell
# 终端1，在 tauri-rust/
npm run dev

# 终端2，在 tauri-rust/
cargo run --manifest-path src-tauri/Cargo.toml -- --port=2427 --adduser=127.0.0.1:2425 --verbose
```

直接运行构建产物时使用相同参数：

```powershell
.\src-tauri\target\debug\SpeedIpMsgRust.exe --port=2427 --adduser=127.0.0.1:2426 --verbose
```

### 启动参数

| 参数 | 说明 |
|---|---|
| `--port=N` / `--port N` | 默认2427；0和非法端口拒绝启动 |
| `--adduser=IP:port[,IP:port...]` | 本次进程的直接发现目标；不写入持久配置 |
| `--verbose` | 增加事件诊断日志 |

设置中的“直接用户”会持久保存；与命令行目标共同参与发现。默认广播发送到2425和本机监听端口，原 C++ 版本若在2426，需直接添加该地址。

Windows首次联网可能提示防火墙权限，请自行确认是否允许局域网访问。本项目不会自动修改防火墙。

## 与原版隔离

- 应用标识：`com.speedipmsg.rustpreview`。
- 默认监听2427，先绑定端口成功后才打开本实例的可写配置/数据库，同端口第二实例明确报错。
- 默认协议用户名：`<系统用户名>-rust-<端口>`，不修改系统账户。昵称可在设置中修改。
- 远端联系人ID包含协议用户名、主机名、IP和监听端口，例如 `feng@FENG#127.0.0.1:2426` 与 `feng@FENG#192.168.2.88:2425` 是独立联系人。用户名/主机名中的分隔符按百分号转义，不修改线上协议身份。临时源端口仅在唯一已知候选时归并，多个候选不会猜测。
- 早期Rust版不含端点的旧历史ID保留在原会话，不删除，也不自动归属到某个新端点；请从通讯录选择新联系人继续会话。
- 数据目录使用 Tauri `app_local_data_dir()/port-<端口>/`，设置页显示实际路径：

```text
config.json       # Rust权威配置，原子替换写入
messages.db       # 独立SQLite历史（WAL）
rust-preview.log  # 桌面事件和错误日志
webview/          # 本实例独立WebView数据
```

不读取 `.speedipmsg*` 下的数据库、不读取旧 `ipmsg-config` IndexedDB、不读取旧 DataDir 注册表。MVP数据目录只读，不支持在线切库或指向原版目录。

清空历史按数据库事务返回的实际删除ID同步界面，不用客户端时间推测删除边界。数据库另外保留不含正文的消息ID去重记录，用于七天内的重传抑制，防止刚清空的消息被UDP重试重新插入；过期ID在后续写入时清理。

## IPC 与启动顺序

前端继续调用 `invoke('domain.action', args)`；桥接层转为 Tauri `ipmsg_command {command,args}`。Rust命令使用白名单。

Tauri 2 原生事件名不允许点号，因此仅订阅 `ipmsg-event`，负载为：

```json
{"event":"message.received","payload":{"id":"...","from":"...","content":"...","type":"text","timestamp":0}}
```

前端先注册本地监听，等待 `bridgeReady()` 的异步原生订阅完成，再读取配置/本机用户/历史并调用幂等的 `config.loaded` 启动发现。保存配置失败不会伪装成功；消息 ACK/失败可能早于命令返回，前端按 messageId 缓冲再应用。

Tauri窗口中不会自动退回mock。浏览器演示仅在明确设置 `VITE_MOCK_BRIDGE=1` 时启用；演示不代表协议已发送消息。

## 验收建议

1. Rust版2427与飞秋2425、原版2426分别配置直接发现，确认真实联系人和组名。
2. 双向发送中文、多行、文本表情及字体尾标消息，确认只展示正文并显示真实回执。
3. 对端退出或不应答，检查消息失败状态；同一报文重发不重复显示/增加未读。
4. 重启检查历史、分页、搜索；清空失败时不显示“成功”。
5. 保存昵称、组名、直接用户；重启仍有效。损坏配置应报错并保留文件，不能静默覆盖。
6. 关闭到托盘后仍接收；托盘恢复可用；托盘退出结束进程；托盘创建失败时不隐藏到无法恢复的状态。
7. 确認原版的目录、配置和历史未被修改。

IPMsg文本在局域网中是明文协议，本MVP不提供身份认证或加密，不应用于不可信网络。
