# MyGo 迁移文档与计划（tauri-rust → MyGo）

> 编写日期：2026-10-29
> 前置文档：[MyGo框架评估.md](./MyGo框架评估.md)（可行性评估）、[tauri-rust/MIGRATION.md](./tauri-rust/MIGRATION.md)（Rust 版全部功能与验收记录）
> 已落地验证：`SpeedIpMsg-Go/` 最小闭环 demo（提交 `6e4633c`）：UDP 发现（飞秋兼容头）→ 联系人列表 → 文本收发（GBK + RECVMSG 回执）→ 未读托盘闪烁，`go test` 回环集成测试通过，产物 9.7 MB exe。
> **当前状态（2026-10-29）**：M0–M6 阶段代码全部完成（最新提交 `ee152bf`）；全量 `go test ./...` 8 个包全绿、`go vet` 干净；产物 exe 13.7 MB + NSIS 5.4 MB。M6 后又修了三处实机问题（身份语义对齐 Rust、扫描写超时、启动失败可见化），见 §9、§10。剩余为 M1–M6 实机验收与发布决策，见 §10。

## 1. 目标

将 Rust 版（`tauri-rust/`，对外名称"迅秋"）的全部已交付功能迁移到 MyGo（Go + 系统 WebView），达到功能对等。迁移遵循 Rust 版迁移时确立的既有约束（见 MIGRATION.md"后续迁移约束"）：不做多网段广播；不读取/迁移旧版（C++ 版与 Rust 版）数据；飞秋图片必须走"先全部分片确认、后发引用"顺序；所有后台任务可取消、锁不跨网络/数据库 await。

**本计划是"从零重写内核、复用前端"**：Go 侧重写约 14.7k 行 Rust 的等价逻辑，前端 React/TS 代码基本原样迁移，只重写 IPC 层。

## 2. 现状基线（迁移对象盘点）

### 2.1 代码规模（2026-10-29 实测）

| 层 | 规模 | 构成 |
|---|---|---|
| `crates/ipmsg-core`（无 UI 内核） | ≈ 9.2k 行 | database 1117 / network 1035 / image/fragments 979 / file/transfer 794+686 / protocol 652 / image_send 595 / dib 451 / config 424 / 其余（scan、lzw、assets、avatar、knock、text…） |
| `src-tauri`（平台层） | ≈ 5.5k 行 | storage 885+584 / capture 663+217 / system_notification 480+282 / runtime 473 / notification 346 / commands 320 / image 191 / files 129 / platform 172 |
| 前端 `src/` | ≈ 4.6k 行 | 21 组件 + 7 store + 6 services + 10 utils；IPC 层 `services/bridge.ts` 294 行是唯一需要重写的部分 |
| 测试基线 | — | 核心单测+集成 135 项、桌面 43 项、前端 78 组 |
| **Go 版实际（2026-10-29）** | ≈ 10.4k 行 + 4.1k 行测试 | `internal/` 9 个包 63 个 `.go` 文件；`main.go` 为 MyGo 框架层；前端沿用 Rust 版 `src/` 整树 |

### 2.2 功能清单（全部需迁移）

文本（GBK/UTF-8、多行、字体尾标清理）、发现（广播/直接用户）、消息回执/重发/去重、SQLite 历史（schema 6、分页/搜索/清空）、配置（JSON 单一来源 + 校验 + 安全替换）、分组通讯录、图片收发（飞秋 LZW 循环字典、DIB 转码、分片状态机、资产库、缩略图、`ipmsg-image` 自定义协议）、截图标注（GDI 捕获、独立编辑窗口、标注工具）、文件传输（TCP 同端口、文件夹 GETDIRFILES、断点续传、多连接并行下载、拖放、不覆盖发布）、IP 段扫描、提示音（MCI）、Windows Toast 系统通知（AUMID + 开始菜单快捷方式 + 点击激活）、飞秋 96 表情 / 84 头像、窗口振动（0xD1）、上下线公告、托盘（闪烁、未读）、登录名/MAC 展示、拼音首字母搜索、数据目录（默认 2425、扁平化、自定义目录、不可用回退）。

## 3. 总体架构映射

| Rust 版 | MyGo 版（建议） | 说明 |
|---|---|---|
| `crates/ipmsg-core/src/protocol.rs` + `text.rs` | `SpeedIpMsg-Go/internal/ipmsg/` | demo 的 `ipmsg.go` 扩展；LZW/兼容头已验证 |
| `network.rs` + `network/*` | `internal/network/`（拆 session / image_receive / image_send / scan / knock） | 同一包内避免 mygo 依赖，保可测性 |
| `database.rs` + `config.rs` | `internal/store/` | SQLite 插件 + JSON 配置 |
| `image/*`（lzw/dib/fragments/assets） | `internal/media/` | LZW/DIB 从 Rust 逐行移植，与现有测试对拍 |
| `file/*` | `internal/filetransfer/` | TCP/文件夹/断点/并行 |
| `src-tauri/capture*` | `internal/platform/capture/` | GDI `BitBlt`+`DwmFlush` 移植，`golang.org/x/sys/windows` 或 purego，无 cgo |
| `src-tauri/system_notification*` | `internal/platform/toast/` | **最大移植项**，见 §5 风险 |
| `src-tauri/notification*`（提示音） | `internal/platform/sound/` | MyGo `audio` 包原生支持 mp3/wav，替代 MCI |
| `src-tauri/storage*` | `internal/storage/` | 纯文件操作，直接移植 |
| `runtime.rs` + `commands.rs`（白名单 IPC） | `main.go` + `mygo.Bind` 结构体 | `mygo generate` 生成类型化 TS 客户端，替代手写分发 |
| `lib.rs`（托盘/窗口/拖放） | `main.go` | `NewTray`/`OnFileDrop`/`OnClose`，API 已核实 |
| 前端 `src/**` | 原样复制 | 仅重写 `services/bridge.ts`（294 行）→ 包装生成的 `mygo.ts`；`services/clipboard.ts` 等 Web API 层不动 |
| 前端 IndexedDB 配置 | 需数据迁移脚本 | origin 从 `ipmsg.localhost` 变为 `mygo.localhost` 形态（见 §5） |

**工程位置**：建议直接在已提交的 `SpeedIpMsg-Go/`上演进为正式工程（改 `mygo.config.ts` 的 name/identifier/版本），不另起目录；demo 的 `ipmsg.go`/`network.go` 平移为 `internal/ipmsg`/`internal/network` 的起点。核心包保持"不 import mygo"，沿用 ipmsg-core 的离线可测设计。

## 4. MyGo 能力清单（已核实的 API 底账）

| 能力 | API | 状态 |
|---|---|---|
| 类型化 IPC | `mygo.Bind` + `mygo generate` → TS 客户端；`NewEvent[T]` 事件 | ✅ demo 已验证 |
| 托盘 | `NewTray{Icon,Menu,ToolTip}` + `SetIcon(png)` + `OnClick` | ✅ demo 已验证（闪烁方案直接平移） |
| 窗口 | `OnClose`(可阻止)/`OnFocus`/`OnBlur`/`IsFocused`/`Show`/`Hide` | ✅ demo 已用 |
| 文件/图片选择 | `Dialog.Open/Save`（含 filter） | API 已核实，M2 接入 |
| 原生拖放 | `Window.OnFileDrop`（返回 paths + 坐标） | API 已核实，M4 接入 |
| 剪贴板 | `ClipboardModule.Read/Write/ReadFiles`（transfer 格式） | API 已核实；粘贴图片优先走 WebView2 Web API（评估稿未验证项） |
| SQLite | 官方插件：purego FFI，**无 cgo**；`mygo build` 打包 `mygo-sqlite3` DLL 进 resources；`Open{BusyTimeout}`/`Execute`/`Query`/`Transaction` | API 已核实；rusqlite 用法（schema 迁移、quick_check、WAL）需在 M1 对拍验证 |
| 音频 | `audio` 包（mp3/wav 解码播放、mixer） | API 已核实；提示音可直接用 `notification.mp3` |
| 系统通知 | 框架自带 Windows 通知区气泡（无通知中心富交互） | 不足以替代 Toast 方案，需自研（§5） |
| 自动更新 | `updater`（feed 检查、下载进度、安装）+ NSIS + 签名 + GitHub Actions 文档 | Rust 版按用户要求暂缓；MyGo 侧成本低，列为可选决策点 |
| 单实例 | `single_instance` | 现成 |
| 打包 | `mygo build` → exe + NSIS 安装包 | ✅ demo 已产出 |

## 5. 风险登记

| # | 风险 | 等级 | 应对 |
|---|---|---|---|
| 1 | **Toast 富交互**：框架通知只是气泡；现有方案依赖通知中心、点击激活回会话、AUMID 快捷方式（`system_notification/identity.rs` 282 行 COM 逻辑） | 高 | 用 purego/x-sys 移植 COM + WinRT（`IShellLinkW`/`IPropertyStore`/`ToastNotifier`）；Rust 版 0x80070490 排障记录（MIGRATION.md）是现成的行为规格；若成本过高，降级为框架气泡并明确告知用户 |
| 2 | **SQLite 插件兼容性**：当前 schema 6 的迁移、`quick_check`、busy 语义、单连接线程模型 | 中 | M1 首个任务做 1:1 对拍（建表→迁移→读写→校验），不满足则在插件外直挂同一 DLL 扩展封装 |
| 3 | **WebView2 粘贴图片**：`clipboardData.files` 的 File 对象是否透传 | 中 | M2 第一项实测；不透传则退 `ClipboardModule.Read` 原生位图 |
| 4 | **截图多屏/DPI**：GDI 移植后 150% 缩放、负坐标副屏矩阵需重测 | 中 | 保留 Rust 版验收清单（MIGRATION.md 下一步验收 5–7）原样回归 |
| 5 | 单人维护框架（v0.7.1）、API 演进 | 中 | 锁定 mygo 版本；核心包不依赖 mygo，框架层薄 |
| 6 | IndexedDB 配置 origin 变化 | 低 | 一次性导入脚本或引导用户重配（配置项少，成本低） |
| 7 | 与 Rust 版同机共存：同名同端口冲突 | 低 | 迁移期 MyGo 版用独立名称/端口开发，验收后再定发布形态 |

**风险现状（2026-10-29 收尾时）**：#1 已按预案降级（应用内提示，能力位 `systemNotifications:false`）；#2 已对拍通过并记录插件限制（手动 `BEGIN/COMMIT` 禁用，条件写入走 `Transaction`）；#3 已验证 WebView2 透传 `clipboardData.files`（改传 base64）；#4 代码完成，多屏混合 DPI 待实机验收；#6 已消除（配置移到 Go 侧 JSON，前端无 IndexedDB）；#7 已隔离（`%LOCALAPPDATA%\SpeedIpMsgGo`，非默认端口追加 `_<端口>`）。

## 6. 分阶段计划

每阶段一轮"实现 → 自动验证 → 实机验收 → 提交"，与 Rust 版迁移节奏一致。阶段顺序刻意与 Rust 版迁移史对齐，验收清单可直接复用 MIGRATION.md"下一步验收"对应条目。

### M1 核心通信（对齐 Rust 阶段一）
- **范围**：协议/文本（demo 已有大半）→ 消息回执重发/去重、SQLite 历史（schema 1→6 一次建全）、配置 JSON、分组通讯录、托盘与关闭行为、设置页骨架
- **关键点**：SQLite 对拍是开门第一项（风险 2）；demo 的 Hub 改造为经 store 持久化；`feiqCompatVersion` 等报文逻辑冻结不动
- **验证**：`go test` 单测 + 双实例回环 + 飞秋实机互发中文；`npm run tauri` 式整包 `mygo build`；前端 78 组测试迁移跑通
- **预估**：2–3 轮

### M2 图片收发（对齐阶段 2A/2B + 剪贴板）
- **范围**：`internal/media` LZW/DIB/分片状态机/资产库移植；收图（2A）→ 选图发送（2B）→ 剪贴板粘贴
- **关键点**：LZW 移植后必须与 Rust 版 + `tools/test_feiq_lzw.cpp` 基准逐像素对拍；保持"分片全确认后发引用"顺序与 0x120 引用双 NUL 格式；`ipmsg-image://` 改为 MyGo 自定义协议（`http://<scheme>.localhost`）；raw IPC 改 base64/临时文件（评估稿已注明的 API 差异）
- **验证**：真实飞秋样本（200×268、394×198）往返像素一致；评估稿未验证项 3（粘贴 File 对象）在此实测
- **预估**：3–4 轮

### M3 截图标注（对齐阶段 2C）
- **范围**：GDI 捕获移植（`capture/platform.rs` 217 行是移植核心）→ 截图窗口/编辑器 → 导入发送链路
- **关键点**：MyGo 无现成截图 API；窗口管理（Preparing/Editing 会话状态机、退出顺序、从不置顶）复用 Rust 版设计；编辑器组件从 `ScreenshotEditor.tsx` 原样迁移
- **验证**：MIGRATION.md 验收 5–7（多屏/DPI/异常注入矩阵）
- **预估**：2–3 轮

### M4 文件传输（对齐阶段三 + 传输增强）
- **范围**：TCP 同端口、文件邀请/回执、文件夹 GETDIRFILES、断点续传（schema `file_receives`）、并行下载与回退、原生拖放（`OnFileDrop`）、选择器（`Dialog.Open`）、不覆盖发布
- **关键点**：`file/*` 约 2.3k 行是内核最大块；来源校验（IP+身份+包号+文件ID）、偏移越界拒绝等安全逻辑逐条对照移植
- **验证**：MIGRATION.md 验收 8–9、15
- **预估**：3–4 轮

### M5 扫描与通知（对齐阶段 4A/4B + 系统通知）
- **范围**：IP 段扫描（惰性迭代、限频事件、scanId/revision 互斥）；提示音（`audio` 包替换 MCI，策略层照搬：开关/焦点/节流/去重）；Toast 系统通知（风险 1，若降级需用户确认）
- **验证**：MIGRATION.md 验收 10–14
- **预估**：2–3 轮

### M6 体验补齐与发布形态（对齐后续各轮）
- **范围**：表情/头像（资源导入脚本改 Go，前端组件原样）、振动 0xD1、上下线公告、登录名/MAC、拼音搜索、托盘闪烁细节（demo 已有）、数据目录（layout/回退/自定义目录）、登录信息页
- **可选决策点**：自动更新（MyGo updater 现成，Rust 版曾暂缓）；与 Rust 版的发布关系（替换/并存）
- **预估**：2–3 轮

### 总量估计
约 **14–20 轮**（Rust 版迁移史共约 20 轮，可作对照）。前端组件层基本零重写；Go 内核 ≈ 9–12k 行新写 + 平台层 ≈ 4–5k 行移植。

## 7. 决策点（2026-10-29 用户已确认）

1. **是否启动**：✅ 启动迁移。
2. **工程位置**：✅ 演进 `SpeedIpMsg-Go/`，不另起目录。
3. **开发期共存**：✅ 开发期用独立名称/端口，避免与 Rust 版实例冲突；验收互通时临时改 2425。
4. **Toast 降级预案**：✅ 保留；风险 1 成本超预期时按预案降级为框架气泡并明确告知。
5. **自动更新**：✅ 维持 M6 可选项，届时再定。
6. **数据边界**：✅ **历史数据不迁移**（用户明确确认）——Rust 版与 C++ 版的数据库/配置/图片资产均不读取；MyGo 版从全新数据目录起步。

**执行约定**：每阶段（M1–M6）完成时更新本文档 §9 进度记录与 MIGRATION.md 相关记录，随阶段代码一并提交。

## 8. 验证策略（贯穿各阶段）

- **核心离线可测**：`internal/*` 不依赖 mygo，`go test ./...` 全覆盖（协议、LZW、数据库、文件、扫描），回环集成测试沿用 demo 模式（两个 Hub + 临时端口）；`tools/fakepeer` 保留为黑盒验收工具。
- **前端**：TS 全量类型检查 + 78 组 store/组件测试迁移；`mygo generate` 产物纳入检查。
- **实机矩阵**：每阶段复用 MIGRATION.md"下一步验收"对应条目；飞秋互通用真实样本（图片像素对拍、中文/换行/表情混排、文件夹含空子目录）。
- **构建链**：`npm run build`（mygo build）出 exe + NSIS；交叉编译验证留 M6。

## 9. 进度记录

| 日期 | 阶段 | 内容 | 状态 |
|---|---|---|---|
| 2026-10-28 | M0 | 最小闭环 demo：报文/发现/文本/回执/托盘闪烁，黑盒验证通过，提交 `6e4633c` | ✅ |
| 2026-10-29 | 决策 | 用户确认启动迁移，6 项决策点全部落定（数据不迁移） | ✅ |
| 2026-10-29 | M1-1 | 代码重构为 `internal/ipmsg`（协议，符号导出）+ `internal/network`（Hub）+ `internal/store`；SQLite 插件对拍通过：purego 无 cgo、WAL/foreign_keys/BusyTimeout/事务可用，但 **`Execute` 不允许手动 BEGIN/COMMIT（SQLITE_AUTH）**，条件写入必须用 `INSERT...SELECT...WHERE NOT EXISTS` 在插件 `Transaction`（BEGIN IMMEDIATE）内表达；配置层（JSON 单一来源、严格类型、null 拒绝、原子替换）与数据库层（schema 6 全量建表、去重/分页/搜索/清空语义）逐条对拍 Rust 版用例；`go test ./...`（26 项）、`go vet`、`npm run build` 全部通过 | ✅ |
| 2026-10-29 | M1-2 | 会话持久化接入 Hub（先落库再发送、`rx:<ip:port>:<包号>` 去重、RECVMSG 置已送达、2s/4s 重发同包号三次后置失败）+ `internal/app` 统一命令分发（`Backend.Command`，M1 命令集：app.info/config/user/message/history/tray，未迁移能力明确报"后续阶段"）+ 用户变更差分事件 + 资料热更新（昵称/分组/头像进版本头）+ React 前端整树迁移（bridge.ts 仅换传输层：`Backend.Command` + 单一信封事件 `ipmsg-event`，mock 保留）；`go test`（含带库回环集成与重复报文重放）、`tsc`、`mygo build`（12.5 MB）通过；打包产物黑盒验证通过（ANSENTRY/RECVMSG + fakepeer 消息正确落库 schema 6） | ✅ |
| 2026-10-29 | M1-3 | M1 收尾：`user.local` 补 success、README 重写、文档更新 | ✅ |
| — | M1 验收 | 实机验收清单见下 | ⏳ 待用户验收（已授权继续 M2） |
| 2026-10-29 | M2-1/2 | `internal/media`：LZW 编解码（循环字典/12 位饱和/LSB-first→MSB-first 字节）+ DIB 容器（LZW!头/CRC32/自底向上 24/32 位 DIB/白底合成）+ CRC32，15 项对拍测试全绿；Go 侧已知差异：无 EXIF 方向解析、BMP 仅 24/32 位 BI_RGB、缩略图用最近邻 | ✅ |
| 2026-10-29 | M2-3 | 分片状态机（19 项对拍）+ 资产库（独占目录/显式 Keep/Release 生命周期）+ 收图链路（单并发解码工作协程、提交闸门持重组器锁、引用先行/后行均一条历史、重放去重）；`ipmsg-image` 自定义协议用 `mygo.Protocol.Handle` 原样替代；回环集成测试通过 | ✅ |
| 2026-10-29 | M2-4 | 发图链路（先落库再入队、32 片窗口 4 轮/末窗 15 轮 30s、全部确认后发引用、引用 4 次重试同包号、120s 总期限、取消/超时/队列上限 4）+ `image.select`（mygo Dialog）/`send`/`cancel`/`discard`/`import_clipboard`（base64 参数）命令 + 进度/完成/失败事件 + 能力位放开；发送全链路回环测试通过（A→B 落库、A 置已送达） | ✅ |
| — | M2 验收 | 实机验收：与真实飞秋互发 PNG/JPEG/BMP 图片核对像素与缩略图；M2 已知限制（缩略图算法、EXIF）记录在案 | ⏳ 待用户验收（已授权继续 M3） |
| 2026-10-29 | M3 | 截图与标注：GDI 捕获移植（`internal/platform/capture`，纯标准库 syscall：SetThreadDpiAwarenessContext/DwmFlush/BitBlt+CAPTUREBLT/GetDIBits，句柄显式释放，显示器枚举走 MonitorFromWindow）+ 截图会话（单一会话额度、5 分钟硬超时、确认/取消/关窗幂等结束、主窗口隐藏恢复）+ 独立无边框置顶编辑器窗口（`/?capture=<id>`）+ `ipmsg-capture` 协议仅服务活动会话 + `screenshot.start`（阻塞至会话结束返回导入资产）/`screenshot.confirm`（base64→`ImportScreenshot`→发图链路）/`screenshot_command`（read/ready/cancel）命令；前端编辑器组件原样复用；**已知限制**：编辑器窗口按 DIP 定位（100% 缩放与 Rust 物理像素一致，多屏混合 DPI 待实机验收核对）；GDI/边界测试对拍通过，全量构建全绿 | ✅ |
| — | M3 验收 | 实机验收：截图编辑器全流程（框选/标注/确认入会话）多屏缩放核对 | ⏳ 待用户验收（已授权继续 M4） |
| 2026-10-29 | M4-1 | 文件协议层：`internal/filetransfer`（邀请解析 16 项上限/8GiB/特殊文件拒绝、文件名净化 180 字符/保留名、TCP 请求解析 hex original:file:offset），`0x60/0x61/0x62` 补进文本帧命令表；对拍测试全绿 | ✅ |
| 2026-10-29 | M4-2 | 传输引擎移植（transfer.rs 795 行 + streams.rs 686 行 + directory.rs + storage.rs）：TCP 与 UDP 同端口、来源身份五元组校验、发送邀请重试（2s×3）与 600s 过期、接受→下载队列（4 并发）、≥4MiB 并行分块按序回填、断点续传（file_receives 检查点 + 重启恢复）、暂停/取消/拒绝 + RELEASEFILES 通知、发送端 60s 空闲判失败、不覆盖发布（MoveFileExW flags=0）/重名绕行、目录流（4096 项/32 层/GBK 头/防穿越）；Go 结构差异：tokio→goroutine、watch→可重置取消通道、RAII Busy→显式 defer；store 层补 `FileMetadata`/message_files/file_receives 读写与 kind==2 历史水合；回环集成测试 5 项（普通/空文件/5MiB 并行/断点续传/拒绝通知）全绿 | ✅ |
| 2026-10-29 | M4-3 | 应用接线：Hub 实现 `filetransfer.Link`（复用 UDP socket/包号/线缆编码），`SENDMSG|FILEATTACH` 路由进引擎（服务未启动时占位文本且不回执）、RECVMSG/RELEASEFILES 先给引擎认领；`file.select/select_folder`（Dialog.Open，与图片选择共用互斥）/`send`/`discard`/`accept`/`resume`/`pause`/`reject`/`cancel`/`open_folder`（explorer /select + CREATE_NO_WINDOW）命令；`file.updated`/`message.received`/`file.receive_failed` 事件（fromUser 换前端 User 形状）；`Window.OnFileDrop` 拖放→当前会话预览（≤8 项，file.selected 事件）；接收目录 `<Downloads>/SpeedIpMsgGo`；退出前引擎 Shutdown；能力位 files 放开。打包 exe 黑盒验证：fakepeer 发现+文本回执通过、文件邀请落库（camelCase 元数据、incoming=true、RECVMSG 确认） | ✅ |
| — | M4 验收 | 实机验收：与真实飞秋/双实例互发文件与文件夹（大文件并行、暂停续传、拒绝、重名发布核对） | ⏳ 待用户验收（已授权继续 M5） |
| 2026-10-29 | M5-1 | IP 段扫描移植（scan.rs + network/scan.rs）：`internal/network/scan.go` 状态机（idle/running/waiting/cancelling/completed/cancelled/failed）、范围合并与 65536 上限（复用 store 层 ParseRange/Plan）、探测 1s 写超时、进度事件 100ms 限频（network.scan_progress/scan_complete）、scanId 互斥 + skipExisting（启动期自动扫描）、取消随时打断、本机地址照常探测（偏离 C++/Rust：**不跳过本机**；自身回环由接收侧 isSelf 过滤）、退出 ShutdownScan；`config.loaded` 收尾触发 AnnounceOnStartup 公告 + ScanOnStartup 自动扫描（对拍 Rust ui_ready）；`network.scan_range/cancel/status` 命令、能力位 scan 放开；Hub 端口 0 时回填实际绑定端口。集成测试 4 项（发现对端完成/取消互斥/非法范围/本机照常探测且不成联系人）全绿 | ✅ |
| 2026-10-29 | M5-2 | 提示音移植（notification.rs + native.rs）：`internal/notification`（内置 notification.mp3 资源、临时缓存文件唯一不覆盖、MCI mciSendStringW/mciGetErrorStringW 播放、15s 时长上限）；策略闸门 NotificationGate（去重 256 条 5 分钟 TTL、1s 冷却、正在阅读会话静音且登记、事件/类型/来源过滤）；单一有界播放工作线程（epoch 作废在途请求、试听 3s 超时只作废当代、队列 1、错误 60s 节流上报 notification.sound_failed）；对拍测试 11 项全绿；`notification.test_sound` 试听命令、config 同步 SetEnabled、message.received 三处入口（文本/图片/文件）接闸门；能力位 notificationSound 放开 | ✅ |
| 2026-10-29 | M5-3 | 系统通知：**按迁移决策 4（用户已确认的降级预案）降级**——WinRT Toast（AUMID 快捷方式注册 + ToastNotifier + 激活回调）需纯 Go 手写整套 COM/WinRT 接口，成本超预期；已移植可移植部分（Notice 文案构造：默认不泄露内容、预览开关、控制字符过滤、8 类文案，2 项对拍测试），能力位 `systemNotifications:false`，前端设置页自动灰置并显示说明（前端已有该降级 UI），`notification.test_system` 返回明确错误；后续如需真系统通知只需补 `native_windows.go` 后端 | ✅（降级） |
| 2026-10-29 | M5 收尾 | 全量 `go test ./...` 全绿；打包 exe 黑盒验证：启动正常（提示音服务初始化）、fakepeer 发现+文本回执通过 | ✅ |
| — | M5 验收 | 实机验收：扫描设置页扫到真实飞秋、收到消息有提示音（试听按钮）、消息持续爆发只响一声、正在阅读不响 | ⏳ 待用户验收（已授权继续 M6） |
| 2026-10-29 | M6-1/2 | 窗口振动 0xD1（knock.rs 对拍：SendKnock 10s 冷却/4096 上限/不重试、receiveKnock 空帧 0-2 NUL 兼容、message.knock 命令启用、ReceivedEvent.Type 供前端特效）；头像与 MAC（avatar.rs + feiq_mac 对拍：ParseAvatar builtin 0..83/custom/unknown、FeiqMac 12/17 位归一化与非法拒绝；User 携带头像/MAC，user.list/userToFrontend 输出真实值；登录名沿用设置页昵称热更新）；拼音搜索/表情/登录信息页为前端组件，随 M1 整树迁移已可用 | ✅ |
| 2026-10-29 | M6-3 | 存储目录管理（storage.rs + layout.rs 对拍）：`internal/storage`（bootstrap 定位文件 data-location.json、数据集身份标记 .speedipmsg-dataset.json、写探针/权限校验、copyTree 20 万项/64 层上限拒绝链接并校验完整性、createDataset/migrate/freshDefault、重启时机迁移 pending 选择 Apply/Cancel、失效自动回退新默认位置并备份原定位记录）；storage.info/select/default/apply/cancel 命令、app.info 增加 storageError；**刻意不移植**旧版目录兼容导入（open_default/open_compatible）——"历史数据不迁移"决策的直接推论。对拍测试 5 项（bootstrap 直用/标记校验与回退/重启迁移/非法目标/命名）；修掉一处 Rust drop 语义差异导致的句柄泄漏（os.Open 未 Close 锁住标记文件）。M6 其余项（表情数据、拼音、托盘细节）随前端整树迁移已可用 | ✅ |
| 2026-10-29 | M6 收尾 | 全量 `go test ./...` 8 个包全绿、`go vet` 干净；自动更新维持 M6 可选项待用户决定 | ✅ |
| — | M6 验收 | 实机验收：振动特效与冷却提示、联系人头像/MAC 展示、存储设置页切换数据目录（重启生效、失败回退） | ⏳ 待用户验收 |
| 2026-10-29 | M6 回归 | app 层命令分发回归测试：`storage.*` 已接线（含默认位置拒绝语义），提交 `ee152bf` | ✅ |
| 2026-10-29 | 收尾状态 | 全部 M0–M6 阶段代码完成；全量 `go test ./...` 8 个包全绿、`go vet` 干净；产物 exe 13.7 MB + NSIS 5.4 MB | ✅ |
| 2026-10-29 | 行为调整 | **扫描不再跳过本机**（按用户要求，刻意偏离 C++/Rust 版 `MsgMng::StartScan`/`scan_worker` 的"跳过自身 IP"）：去掉 `scanWorker` 的 `skip` 分支与 `localIP` 记账（`Hub.localIP`、`SetLocalIP`、`ipv4Text`、`Runtime.New` 里的注入一并删除）；自身回环报文仍由接收侧 `isSelf` 丢弃，不会把自己加进联系人。`ScanStatus.skipped` 字段保留（恒 0）以维持与 Rust 版前端类型/校验的一致，前端进度行不再显示"跳过本机"。`scan_test.go` 的跳过用例改为 `TestScanProbesOwnAddress`（断言 Current=1、Skipped=0、联系人仍为 0） | ✅ |
| 2026-10-29 | 缺陷修复 | **改名后被当成两个人**：协议 `username` 字段旧实现塞的是**昵称**（`buildWire`/图片报文都取 `p.nick`/`h.Nick()`）。对端（含 Rust 版）按 `endpoint_id(packet.username, hostname, addr)` 建联系人，改名即换身份 → 通讯录里多出一个人。修法：`profile` 新增 `login`（稳定登录名，`localLogin(port)` = 系统 `USERNAME`，非默认端口加 `-<port>`，**对拍 Rust `local_names`**），`buildWire`/`buildImageWire`/图片引用报文改用它，昵称只留在 profile 报文 body；`localUser` 的 `username` 输出稳定登录名。新增回归测试 `TestRenameKeepsWireIdentity`（改名前后 `username` 不变、body 跟着变；已确认改前会复现 `旧名 -> 新名`） | ✅ |
| 2026-10-29 | 缺陷修复 | **发送报 `write udp4 ... i/o timeout`**：`scan.go` 的 `sendProbe` 给**共享的** UDP 连接设了 1 秒写超时却从不清除，扫描跑过之后（含启动期自动扫描）所有 `conn.WriteToUDP` 都撞在已过期的 deadline 上立即失败——表现为"扫描后发消息必失败"。修法：`sendProbe` 写完立即 `SetWriteDeadline(time.Time{})` 还原，`sendWire` 再兜底清一次。新增回归测试 `TestScanRestoresWriteDeadline`（先复现失败，再验证修复；Rust 版用 `tokio::time::timeout` 不碰 socket 超时，故此为 Go 移植特有） | ✅ |
| 2026-10-29 | 缺陷修复 | **Win10 双击无反应**：GUI 子系统（无控制台）下 `log.Fatalf`/panic 全部静默丢失，`NewWindow` 返回 nil 也被静默使用。新增 `fatal.go`/`fatal_windows.go`/`fatal_other.go`：日志写 `<数据目录>\SpeedIpMsg-Go.log`（每次启动截断）+ `MessageBoxW` 弹框 + 启动早期 `mygo.WebViewAvailable()` 检查（缺 WebView2 直接提示下载地址，NSIS 不捆绑该运行时）；`main` 加 recover。故障注入验证（库文件不可用 → 日志 + 模态框）。**联系人显示机器名**：飞秋/C++ 版把登录名（常为机器名）放 `username`、昵称放 profile 报文 body，旧实现直接取 `username` 当显示名。`User` 拆分 `Nick`(body)/`UserName`(协议字段)，文件/图片来源校验改用 `UserName`，ID 仍由 `username@hostname#ip:port` 构造；新增回归测试 `TestProfileBodyIsNicknameNotUsername` | ✅ |
| 2026-10-29 | 缺陷修复 | **设置里试听提示音报错**（连报两条：`后端未确认提示音播放`、`提示音设备错误 289：指定的别名已被应用程序占用`）：两个独立缺陷。① `notification.test_sound` 返回 `{success:true}` 没带 `durationMs`，而前端 `services/notification.ts` 拿 `durationMs` 校验（对拍 Rust `{"success":true,"durationMs":duration}`），永远抛"后端未确认提示音播放"——补回 `durationMs`。② MCI 设备句柄绑定在**打开它的那个 OS 线程**上，播放工作协程被 Go 调度器搬走后 `stop`/`close` 报 263（设备未打开）、重复 `open` 报 289（别名被占用）；Rust 侧 `std::thread::Builder::spawn` 是固定线程所以没这问题。修法：工作协程启动即 `runtime.LockOSThread()`（`defer Unlock`），整个生命周期钉在同一线程。新增回归测试 `TestRepeatedPreviewKeepsWorking`（连续 3 次试听，无声卡环境自动 skip） | ✅ |
| 2026-10-29 | 缺陷修复 | **收到消息时任务栏按钮不闪**（Rust 版正常）：先试 `mygo.Window.FlashFrame(true)`，其平台层发 `FLASHW_ALL \| FLASHW_TIMERNOFG`（连标题栏一起、闪到窗口回前台为止），实测收不到提示。改为新增 `internal/app/flash_windows.go` 直接用 `syscall` 调 Win32 `FlashWindowEx`，标志照抄 Rust tao 的 `UserAttentionType::Informational`（`FLASHW_TRAY` + `dwCount=4`，闪 4 次自行停止）；非 Windows 由 `flash_other.go` 空实现。主窗口句柄在 `SetMainWindow` 时取一次存 `r.mainHwnd`（`NativeHandle()` 要回 UI 线程），闪烁不再跨线程。`flash_windows_test.go` 校验 `FLASHWINFO` 布局（32 字节、`hwnd` 偏移 8）——布局写错时 `FlashWindowEx` 只静默返回 FALSE。用户实测通过 | ✅ |

### M1 实机验收清单（对齐 Rust 阶段一）

1. 启动后联系人出现（真实飞秋/另一个实例 `--port=2435`），双实例互发中文/多行/表情符号文本。
2. 重启后历史保留，发送中的消息置失败；同名联系人/同机多实例不串会话。
3. 对端离线发送 → 约 14 秒后置失败并有提示；重复报文不产生重复消息。
4. 设置页修改昵称/分组/头像并保存，重启后保留且对外公告使用新资料。
5. 托盘闪烁/隐藏/恢复、清空会话历史、搜索关键字可用；图片/文件/截图按钮提示"将在后续阶段迁移"（M1 时点；M2–M5 完成后已全部可用）。

## 10. 当前状态与剩余事项（2026-10-29）

**已完成**：M0 最小闭环 + M1–M6 全部阶段代码。Go 非测试代码约 10.4k 行、测试约 4.1k 行（`internal/` 9 个包），`go test ./...` 8 个包全绿；产物 `SpeedIpMsg-Go/build/windows-amd64/SpeedIpMsg-Go.exe` 13.7 MB + NSIS 安装包 5.4 MB。

**剩余事项**：

1. **实机验收 M1–M6**（发布前必须完成）：真实飞秋互通、图片像素对拍、多屏/150% 缩放截图、大文件并行与断点续传及文件夹收发、扫描与提示音、振动/头像/MAC、数据目录切换。清单见 §9 各阶段「验收」行。
2. **系统通知维持降级**（能力位 `systemNotifications:false`）；如需真 Toast，补 `native_windows.go`（COM/WinRT 全手写）。
3. **发布决策未定**：自动更新是否启用；MyGo 版与 Rust 版是替换还是并存（Rust 版目前仍是发布形态）。
4. `SpeedIpMsg-Go/README.md` 仍停在 M1 描述，验收前需按实际能力重写。
5. **已知差异/限制**：缩略图用最近邻缩放、无 EXIF 方向解析、BMP 仅 24/32 位 BI_RGB；截图编辑器窗口按 DIP 定位，多屏混合 DPI 待实机核对；IP 段扫描**不跳过本机**（按用户要求偏离 C++/Rust 版；自身回环由接收侧 `isSelf` 过滤，不会把自己加进联系人）；旧版目录兼容导入刻意不移植（数据不迁移决策）。

**M6 后加固（2026-10-29，均为 M6 完成后的实机问题修复）**：

- 身份语义对齐 Rust：协议 `username` 字段改为稳定登录名（`localLogin`），昵称只走 profile 报文 body；此前把昵称写进 `username`，导致改昵称后对端（含 Rust 版）按 `username@hostname#ip:port` 建键时把同一个人当成两个。
- 修掉扫描残留写超时污染共享 UDP 连接（"扫描后发消息必报 `i/o timeout`"）。
- 启动失败可见化：GUI 子系统无控制台，`fatal()` 落日志到 `<数据目录>\SpeedIpMsg-Go.log` + `MessageBoxW` 弹框，启动早期检查 WebView2 运行时（Win10 可能未装）。
- **收图失败修复**（Go 专用，Rust 无此问题）：`image.read` 曾返回裸 `ipmsg-image://localhost/<id>`，而 WebView2 不认自定义 scheme（mygo 内部把自定义 scheme 映射到 `http://<scheme>.localhost/` 才可加载），导致资产已落盘、气泡有尺寸但 `<img>` 一律加载失败。新增 `schemeURL()`：Windows 返回 `http://<scheme>.localhost/<path>`，其它平台返回 `<scheme>://localhost/<path>`（对拍 Rust `asset_url` 的 `cfg!(windows)` 分支）；`image.read` 与截图 `read` 两条地址均改用它。
- **纯图片引用不再当文本显示**：`CmdSendMsg` 曾先 `receiveText` 再查 `ParseReference`，把 `/~#>id<B~{/font;…}` 原样落成一条聊天文本 + 未读。改为引用判定在前（对拍 Rust `parse_reference` 分支）：命中即登记引用、按 `SENDCHECKOPT` 回 RECVMSG 后 `return`，不产生文本消息也不计未读。回归测试 `TestPureImageReferenceCreatesNoTextMessage`、`TestLoopbackImageReceive`（历史应只含图片记录）。
- **收到消息时任务栏按钮不闪**（Rust 版正常）：Go 版曾用 `mygo.Window.FlashFrame(true)`，其平台层发的是 `FLASHW_ALL | FLASHW_TIMERNOFG`（连标题栏一起闪、闪到窗口回前台为止），实测收不到提示。改为新增 `internal/app/flash_windows.go` 直接调 Win32 `FlashWindowEx`，标志照抄 Rust tao 的 `UserAttentionType::Informational`（`FLASHW_TRAY` + `dwCount=4`，闪 4 次自行停止），非 Windows 由 `flash_other.go` 空实现；主窗口句柄在 `SetMainWindow` 时取一次存 `r.mainHwnd`，避免每次闪烁回 UI 线程取句柄。`flash_windows_test.go` 校验 `FLASHWINFO` 结构体布局（32 字节、`hwnd` 偏移 8），布局写错时 `FlashWindowEx` 只会静默返回 FALSE。用户实测通过。
- **设置里试听提示音失败**（Go 专用，Rust 无此问题）：两处独立缺陷。① `notification.test_sound` 只返回 `{success:true}` 没带 `durationMs`，前端按 Rust 契约校验 `durationMs`，于是永远抛"后端未确认提示音播放"。② MCI 设备句柄绑定在打开它的 OS 线程上，播放工作协程被 Go 调度器搬走后 `stop`/`close` 报 263（设备未打开）、重复 `open` 报 289（别名被占用）；Rust 用 `std::thread::Builder::spawn` 固定线程故无此问题。修法：工作协程启动即 `runtime.LockOSThread()`。回归测试 `TestRepeatedPreviewKeepsWorking`（连续 3 次试听）。
