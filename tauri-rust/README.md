# 迅秋 Rust 预览版（Rust + Tauri 2）

这是旧 C++/TauriCPP 项目旁边的**独立重写试验**，目前包含核心文本通信、第二阶段2A/2B图片收发及2C截图标注。2C已通过自动检查，用户已确认功能验证正常；完整极端场景矩阵仍按清单回归，不是完整功能迁移。

- 原 C++ 源码、构建脚本、数据库及注册表设置不修改、不共享。
- React/TypeScript/Tailwind/Zustand 界面从原工程复制后独立适配；后端是 Rust，没有调用旧 exe/DLL 代替实现。
- Rust 协议、UDP、SQLite、配置逻辑位于 `crates/ipmsg-core`，不依赖 Tauri，可独立测试。
- Tauri 2 壳位于 `src-tauri`，负责窗口、托盘、平台目录、网卡枚举及 IPC。

## 当前状态

**已通过离线检查：**

- Rust核心72个单元测试通过；5个收图/存储、7个发送/截图导入、8个原有UDP集成测试通过（共92项）；桌面截图模块5个单元测试通过。
- `tools/test-store.cjs`：13组前端消息store逻辑测试通过，包括图片元数据、早到终态、取消及清空（IPC使用测试替身）。
- `tools/test-screenshot.cjs`：6组截图坐标与发送流程测试通过；前端完整类型检查和生产构建通过。测试不启动截图窗口，不代表DPI/窗口实机验收。
- 两个真实飞秋LZW样本已验证接收像素；本轮394×198真实样本的发送编码往返也与原始BMP所有像素一致。
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

暂不支持：TCP文件传输、文件拖放、IP范围扫描、自定义网段、提示音、旧版数据导入。文件按钮仍禁用；普通文件不会自动接受。2A实现收图和历史，2B新增选图发送、进度与取消；用户已确认2B功能验证正确。2C新增Windows单屏截图与标注，使用下面的实机清单验收。详见 [MIGRATION.md](MIGRATION.md)。

### 2A 图片接收

- 支持经过长度、CRC和像素边界验证的飞秋LZW DIB，以及经过解码验证的原始PNG/JPEG；统一保存为PNG。
- 分片按端点联系人ID和wire图片ID隔离；最终片在解码、写盘和数据库提交成功后才ACK。重复引用/分片不会重复创建图片消息，不等待引用才能ACK。
- 图片资产位于本实例 `images/<assetId>/full.png` 与 `thumb.png`。历史保存元数据，不存整图base64；2A加入图片表，2B已将SQLite schema升级为3，旧文本历史保留。
- 前端通过 `image.read {assetId,thumbnail}` 获得受控 `ipmsg-image` 图片URL，不能传任意磁盘路径。只有主窗口的本地页面可以访问该协议。
- 发送能力与接收能力分开：当前 `imageReceive=true`、`imageSend=true`、汇总`images=true`；`screenshot`仅在Windows桌面版启用，显式浏览器mock保持禁用。
- 线上报文使用与原C++版一致的飞秋兼容扩展头，避免被飞秋当作不支持图片的普通IPMsg；应用自身版本不变，未实现的文件/加密功能不因扩展头而启用。若飞秋仍提示“对方使用飞鸽”，请重启新版Rust并在飞秋刷新联系人/重新打开对应会话，避免旧客户端类型缓存。`--verbose`日志可看到 `Discovery wire version=1_lbt6_0...`。
- 单载荷16MiB、DIB64MiB、1600万像素、4个活动拼接/32MiB压缩预算；后台解码串行执行。坏图、冲突或超时不会作为成功图片展示。
- 清空历史暂不删除图片文件，避免误删仍被引用的资产；不提供自动图片GC。异常退出可能留下未引用资产，后续将增加可审查的清理流程。

### 2B 图片选择与发送

- 点击图片按钮，通过原生选择器选择PNG/JPEG/BMP，预览加载成功后确认发送。只接受本地20MiB以内的真实图片，透明像素发送时合成白底；暂不支持动态图。
- 发送使用已登记assetId，不接受任意路径参数。取消预览丢弃未引用导入资产；确认排队后资产升级为历史资产，即使取消发送也保留失败记录和图片。
- 先传512字节分片并等待全部0xC1确认，再发送单个0x120图片引用（两个末尾NUL）；引用重试保留同一包号，不自动退回文件传输。
- 4个排队/活动任务；普通窗口最多首发加3次重试，最终窗口最多30秒/15轮，总任务期限120秒。编码、导入和接收共享有界编解码额度。
- 气泡显示排队、编码、传输、等待引用确认等阶段，可取消未封存的任务。取消不等于撤回对端已收到的数据；接口返回成功只表示接受任务，送达依赖实际回执。
- schema 3增加临时资产标记。未发送预览超过10分钟可清理，启动清理旧预览；删除失败保留隐藏的清理标记，后续重试，避免无限积累。已发送/已接收的历史图片不在此自动清理范围内。
- 新命令：`image.select`、`image.send {target,assetId}`、`image.cancel {messageId}`、`image.discard {assetId}`。文件操作仍拒绝。

### 2C 截图与标注

- 聊天工具栏点击截图，暂时隐藏主窗口，只捕获其所在显示器。独立编辑窗口支持选区/全屏、矩形、箭头、画笔、马赛克、中文文字、撤销与重新选区；Enter发送、Esc取消，输入法组词时不触发快捷键。
- 编辑窗口独立全屏、临时置顶，主窗口置顶属性不改动。捕获使用物理像素和显示器原点，画布按显示尺寸换算坐标；不拼接多屏。
- 全屏原图仅保存在当前会话内存，经`ipmsg-capture`协议供对应窗口读取；不写入聊天历史。确认后的PNG导入受管assetId，再复用`image.send`发送到点击截图时的联系人。发送失败按既有气泡状态展示；排队失败清理未引用资产。
- 同时仅一个截图/图片选择操作；屏幕上限1600万像素，选区PNG上限20MiB。初始化20秒、编辑器加载15秒（包含在初始化期限内）、导入30秒、编辑15分钟超时；取消/超时后迟到的窗口和资产仍会清理。
- 取消、关闭、加载/导入失败均关闭编辑窗口并恢复主窗口；退出程序时只清理，不恢复主窗口。截图窗口只可读取、确认、取消自己的会话，不能调用聊天历史、文件或网络命令。
- **待实机验收**：100%/150%缩放、负坐标副屏、中文输入标注；确认发送、Esc、Alt+F4、断网发送失败、编辑中退出后无残留置顶或隐藏窗口；飞秋/C++/Rust对端直接预览及重启历史。

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
npm run test:screenshot
cargo test --offline --locked --manifest-path src-tauri/Cargo.toml --lib

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
