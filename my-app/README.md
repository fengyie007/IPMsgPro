# 迅秋 MyGo 版（迁移中）

用 [MyGo](https://mygo.egoist.dev/)（Go + 系统 WebView）重写 tauri-rust 版迅秋。
迁移计划、阶段划分与进度见仓库根 [MyGo迁移计划.md](../MyGo迁移计划.md)；
Rust 版功能基线见 [tauri-rust/MIGRATION.md](../tauri-rust/MIGRATION.md)。

**当前进度 M1（核心通信）**：UDP 发现（飞秋兼容头）、文本收发（GBK/UTF-8、回执、
三次重试、去重）、SQLite 持久化（schema 6）、JSON 配置、React 前端（自 tauri-rust
整树迁移，IPC 换 mygo 传输层）、托盘与未读闪烁。图片/文件/截图/通知按计划在
M2–M5 迁移，对应入口当前返回"将在后续阶段迁移"。

## 构建

```powershell
cd my-app
go test ./...          # 协议/网络（含回环集成）/存储 全部测试
npm install
npm run build          # mygo build → build/windows-amd64/my-app.exe + NSIS 安装包
```

## 运行

```powershell
.\build\windows-amd64\my-app.exe                 # 默认 2425（与飞秋一致，被占用时需换端口）
.\build\windows-amd64\my-app.exe --port=2435     # 第二实例必须换端口
```

- 数据目录：`%LOCALAPPDATA%\SpeedIpMsgGo`（非默认端口追加 `_<端口>`），
  与 Rust 版（`SpeedIpMsg`）完全隔离；**历史数据不迁移**（用户决策）。
- 启动即广播 BR_ENTRY，收到 ANSENTRY 的对端进入联系人列表；
  点击联系人发消息，收到的消息自动落库并回执。
- 关闭窗口隐藏到托盘；有未读且窗口未聚焦时托盘图标灰/彩交替。
- 托盘菜单：打开主窗口 / 退出（广播 BR_EXIT）。

## 结构

- `internal/ipmsg/` — IPMsg 报文编解码（对齐 tauri-rust `protocol.rs`）
- `internal/network/` — Hub：发现/收发/回执/重试/未读
- `internal/store/` — SQLite（mygo sqlite 插件，purego 无 cgo）+ JSON 配置 + 扫描校验
- `internal/app/` — 命令分发层（对应 Rust commands.rs），M1 命令集
- `src/` — React 前端（自 tauri-rust 整树迁移），`services/bridge.ts` 换 mygo 传输层
- `tools/fakepeer/` — 黑盒验证工具：发现应答 + 文本回执

## 验证

```powershell
go test ./...                                      # 含双 Hub 回环"发现→文本→回执→落库→去重"
go run ./tools/fakepeer -target=127.0.0.1:2425     # 对打包产物黑盒验证
```

浏览器演示：`VITE_MOCK_BRIDGE=1 npm run dev:web`（mock 桥不联网、不写盘）。
