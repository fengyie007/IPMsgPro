# MyGo 迁移文档与计划（tauri-rust → MyGo）

> 编写日期：2026-10-29
> 前置文档：[MyGo框架评估.md](./MyGo框架评估.md)（可行性评估）、[tauri-rust/MIGRATION.md](./tauri-rust/MIGRATION.md)（Rust 版全部功能与验收记录）
> 已落地验证：`my-app/` 最小闭环 demo（提交 `6e4633c`）：UDP 发现（飞秋兼容头）→ 联系人列表 → 文本收发（GBK + RECVMSG 回执）→ 未读托盘闪烁，`go test` 回环集成测试通过，产物 9.7 MB exe。

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

### 2.2 功能清单（全部需迁移）

文本（GBK/UTF-8、多行、字体尾标清理）、发现（广播/直接用户）、消息回执/重发/去重、SQLite 历史（schema 6、分页/搜索/清空）、配置（JSON 单一来源 + 校验 + 安全替换）、分组通讯录、图片收发（飞秋 LZW 循环字典、DIB 转码、分片状态机、资产库、缩略图、`ipmsg-image` 自定义协议）、截图标注（GDI 捕获、独立编辑窗口、标注工具）、文件传输（TCP 同端口、文件夹 GETDIRFILES、断点续传、多连接并行下载、拖放、不覆盖发布）、IP 段扫描、提示音（MCI）、Windows Toast 系统通知（AUMID + 开始菜单快捷方式 + 点击激活）、飞秋 96 表情 / 84 头像、窗口振动（0xD1）、上下线公告、托盘（闪烁、未读）、登录名/MAC 展示、拼音首字母搜索、数据目录（默认 2425、扁平化、自定义目录、不可用回退）。

## 3. 总体架构映射

| Rust 版 | MyGo 版（建议） | 说明 |
|---|---|---|
| `crates/ipmsg-core/src/protocol.rs` + `text.rs` | `my-app/internal/ipmsg/` | demo 的 `ipmsg.go` 扩展；LZW/兼容头已验证 |
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

**工程位置**：建议直接在已提交的 `my-app/`上演进为正式工程（改 `mygo.config.ts` 的 name/identifier/版本），不另起目录；demo 的 `ipmsg.go`/`network.go` 平移为 `internal/ipmsg`/`internal/network` 的起点。核心包保持"不 import mygo"，沿用 ipmsg-core 的离线可测设计。

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
2. **工程位置**：✅ 演进 `my-app/`，不另起目录。
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

### M1 实机验收清单（对齐 Rust 阶段一）

1. 启动后联系人出现（真实飞秋/另一个实例 `--port=2435`），双实例互发中文/多行/表情符号文本。
2. 重启后历史保留，发送中的消息置失败；同名联系人/同机多实例不串会话。
3. 对端离线发送 → 约 14 秒后置失败并有提示；重复报文不产生重复消息。
4. 设置页修改昵称/分组/头像并保存，重启后保留且对外公告使用新资料。
5. 托盘闪烁/隐藏/恢复、清空会话历史、搜索关键字可用；图片/文件/截图按钮提示"将在后续阶段迁移"。
