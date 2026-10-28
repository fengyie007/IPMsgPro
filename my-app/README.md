# MyGo IPMsg 最小闭环 Demo

用 [MyGo](https://mygo.egoist.dev/)（Go + 系统 WebView）复刻 IPMsg Pro 的最小闭环：
**UDP 发现 → 联系人列表 → 单会话文本收发（含 RECVMSG 回执）→ 未读托盘闪烁**。
用于验证《MyGo框架评估.md》中"低成本试水"路径，不是产品代码。

## 构建

```powershell
cd my-app
go test ./...          # 协议单测 + 回环集成测试
npm install
npm run build          # 产出 build/windows-amd64/my-app.exe（约 9.7 MB）+ NSIS 安装包
```

## 运行

```powershell
.\build\windows-amd64\my-app.exe                 # 默认 2425 端口（与飞秋一致，占用时需换端口）
.\build\windows-amd64\my-app.exe --port=2435     # 任意端口；第二实例必须换端口
```

参数：`--port`（默认 2425）、`--name`（昵称，默认 MyGoDemo）、`--group`（分组，默认 MyGo）。

- 启动即向所有网卡广播 BR_ENTRY（飞秋兼容版本头 `1_lbt6_0#128#...`，与 Rust 版同款）；
  收到 ANSENTRY 的对端出现在左侧联系人列表。
- 点击联系人 → 输入框回车发送；线上默认 GBK 编码，对端公告 UTF-8 能力则用 UTF-8。
- 收到消息自动回复 RECVMSG；发出的消息在收到回执后显示"已送达"。
- 关闭窗口隐藏到托盘；有未读且窗口未聚焦时托盘图标彩色/灰色交替（500ms），打开会话或聚焦后停止。
- 托盘菜单：打开主窗口 / 退出（退出会广播 BR_EXIT）。

## 与飞秋/原版互通

协议报文（版本头、常量、GBK 编码、body\0extra\0 分段）逐项对照
`tauri-rust/crates/ipmsg-core/src/protocol.rs` 实现，发现和文本消息应可与
真实飞秋、原 C++ 版、Rust 版互通。图片/文件/振动等扩展能力不在 demo 范围。

## 已知边界

- 消息历史只在内存，退出即失；无去重、无重发。
- 用户以 来源IP:端口 为键，同机多实例互不合并。
- 默认 2425 被占用（飞秋/迅秋已运行）时启动报错，用 `--port` 换端口。
- 评估稿中尚未验证的三项（WebView2 粘贴图片 File 对象、SQLite 插件、实际内存占用）
  本 demo 不涉及，仍待后续验证。
