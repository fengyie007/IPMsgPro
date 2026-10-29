# MyGo 桌面框架评估（对比当前 Tauri + Rust 实现）

> 评估日期：2026-10-28  
> 对象：https://mygo.egoist.dev/（v0.7.1，MIT）  
> 目的：判断是否用 MyGo 替代当前 `tauri-rust/` 实现，以及迁移时各功能点的对应关系。
>
> **后续状态（2026-10-29）**：评估结论被采纳，用户决定启动迁移；M0–M6 阶段代码已全部完成，M1–M6 实机验收待执行。进度见 [MyGo迁移计划.md](./MyGo迁移计划.md) §9、§10。本文保留评估时点的判断，§6 的未验证项已标注实际结论。

## 1. 框架定位

- **Go 程序 + 系统 WebView**（Windows 用 WebView2）承载页面；也可以完全不用前端，用 Go 直接写原生 UI（GPU 绘制，无 webview）。
- **纯 Go、无 cgo**：可从任意机器交叉编译三平台产物，不需要 MSVC / VS Build Tools / Rust 工具链。
- 官网数据：二进制约 7 MB；带 webview 常驻内存约 62 MB，空闲 CPU 0%。
- 前端 IPC：`mygo.Bind(Go 结构体)` + `mygo generate` 生成类型安全 TypeScript 客户端，替代手写命令分发。

## 2. 功能对照（IPMsg Pro 需要的部分）

| 需求 | MyGo 支持情况 |
|---|---|
| UDP 广播 / TCP 文件传输 / IP 段扫描 | Go 标准库，比 Rust 侧更直接 |
| 托盘图标 + 菜单 + 点击 | `mygo.NewTray`；`Tray.SetIcon` 可直接换图，灰/彩色闪烁可原样平移，比 Tauri 简单 |
| 系统通知 | 有；Windows 为通知区气泡，无通知按钮等富交互（迁移实际按降级预案实现：应用内提示 + 能力位关闭） |
| 剪贴板/图片、拖拽文件路径 | 有原生 API（见第 3 节映射） |
| SQLite | 官方插件（纯 Go 驱动） |
| 文件/图片选择对话框 | `mygo.Dialog.Open/Save` |
| 截图/标注 | **无现成 API**，需自行调用 Win32（syscall/purego） |
| 打包/安装器/自动更新 | NSIS 安装包、代码签名、签名自动更新、GitHub Actions 均有 |
| 前端 IPC | `mygo.Bind` + 生成的 TS 客户端，替代当前 `ipmsg_command` 手写分发 |

## 3. 现有 Rust 实现的 API 与 MyGo 对应关系

### 3.1 剪贴板 / 图片（当前实现）

| 功能 | 当前 Rust 版 API | 位置 |
|---|---|---|
| 粘贴图片 | 前端 `ClipboardEvent.clipboardData.files`（WebView2 原生粘贴） | `tauri-rust/src/components/ChatPanel.tsx:242` |
| 过滤图片类型 | `file.type` 判断 PNG/JPEG/BMP + 20 MiB 上限 | `tauri-rust/src/services/clipboard.ts` |
| 二进制传给后端 | `invoke('import_clipboard_image', Uint8Array)`，raw IPC body（`tauri::ipc::InvokeBody::Raw`） | `tauri-rust/src-tauri/src/image.rs:11` |
| 接收并入库 | `Network::import_clipboard` → `image_codec` 转码 | `tauri-rust/crates/ipmsg-core/src/network/image_send.rs:135` |
| 选图片文件 | `tauri-plugin-dialog`：`add_filter("图片", &["png","jpg","jpeg","bmp"]).pick_file(...)` | `tauri-rust/src-tauri/src/image.rs:55` |
| 复制文本 | 纯 Web API：`navigator.clipboard.writeText`、`ClipboardEvent.clipboardData.setData` + `document.execCommand` | `ContactDetails.tsx:19`、`MessageText.tsx:8`、`utils/chatEditor.ts` |
| 图片显示 | 自定义协议 `ipmsg-image://` | `lib.rs:99` |

- 未使用任何 Rust 剪贴板库（无 `arboard` / `clipboard-win`），剪贴板内容全部由 WebView2 的 Web API 获取。
- **迁移含义**：前端粘贴图片逻辑可原样保留；`import_clipboard_image` 的 raw body 不能照搬，MyGo 绑定为 JSON 参数，需改传 base64 或临时文件路径。

### 3.2 拖拽文件（当前实现）

| 功能 | 当前 Rust 版 API | 位置 |
|---|---|---|
| 接收拖入路径 | `Builder::on_webview_event` + `tauri::WebviewEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. })` | `lib.rs:137` |
| 校验来源 | `webview.url()` + `capture::local_url()` | `lib.rs:140` |
| 处理路径 | `files::dropped()` → `FileTransfers::select_path()` → emit `file.selected` | `files.rs:51` |
| 前端 | 仅 `preventDefault`，路径完全来自 Rust 事件 | `App.tsx:41` |

- **迁移含义**：换成 MyGo 的 `onFileDrop(listener)`（返回 `paths`），处理逻辑一一对应。

### 3.3 截图（当前实现）

- 屏幕捕获使用 `windows-sys` 的 GDI `BitBlt` + `DwmFlush`，无第三方截图库：`tauri-rust/src-tauri/src/capture/platform.rs:57,126`。
- 截图预览走自定义协议 `ipmsg-capture://`。
- **迁移含义**：MyGo 无截图 API，需用 `golang.org/x/sys/windows` 或 purego 重写 GDI 部分，是移植中最重的一块。

## 4. 风险

1. **v0.7.1，单人维护（egoist）**，API 仍在演进，出问题只能看源码。
2. 生态小，没有 Tauri 那样的插件市场与社区存量。
3. Windows 自定义协议为 `http://<scheme>.localhost`，IndexedDB/localStorage 的 origin 与 Tauri 不同，前端本地配置（`ipmsg-config`）需迁移。—— **已消除**：配置改由 Go 侧 JSON 保存（`internal/store`），`SpeedIpMsg-Go/src/` 内已无 IndexedDB/localStorage 引用。
4. 迁移本质是**重写 `ipmsg-core`（Rust → Go）**，前端基本不动，工作量约等于现有 Rust 内核全部代码。

## 5. 结论

- 痛点若是**构建链**（VS Build Tools、Rust 工具链、签名流程），MyGo 收益直接：`go build` 出 7 MB 级 exe，且可交叉编译。
- 若只是常规功能迭代，**不建议中途迁移**：现有 Tauri 版本可运行，重写内核的收益不足以覆盖成本。
- 低成本试水路径：用仓库里已生成的 `SpeedIpMsg-Go/` 脚手架跑通最小闭环（UDP 发现 + 一个会话 + 托盘闪烁），再决定是否整体迁移。

**后续（2026-10-29）**：用户已决定启动迁移，工程在 `SpeedIpMsg-Go/` 上演进（不另起目录）。M0–M6 阶段代码全部完成：Go 非测试代码约 10.4k 行、测试约 4.1k 行（`internal/` 9 个包），产物 `SpeedIpMsg-Go.exe` 13.7 MB + NSIS 安装包 5.4 MB。Rust 版仍是当前发布形态；MyGo 版剩余实机验收与发布决策。

## 6. 尚未验证

- MyGo 的 WebView2 是否透传粘贴图片的 `File` 对象（文档只说明页面剪贴板可用）。—— **已验证**：透传可用，前端继续用 `clipboardData.files`，改以 base64 调用 `image.import_clipboard`（M2-4）。
- WebView2 在 Win10/Win11 上的实际内存占用、安装包体积、多网卡行为。—— **部分验证**：安装包 5.4 MB、exe 13.7 MB 实测；内存占用与多网卡行为仍未测。
- 官方 SQLite 插件是否满足当前查询写法。—— **已验证**：purego 无 cgo，WAL/事务/BusyTimeout 可用；限制是 `Execute` 不允许手动 `BEGIN/COMMIT`（SQLITE_AUTH），条件写入须在插件 `Transaction`（BEGIN IMMEDIATE）内用 `INSERT...SELECT...WHERE NOT EXISTS` 表达（M1-1）。
