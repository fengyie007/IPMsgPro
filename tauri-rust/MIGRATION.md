# Rust 重写迁移记录

## 阶段一：核心可用版

本阶段只在 `tauri-rust/` 新增文件。旧 C++ 项目继续作为完整功能版本，不进行原地替换或数据迁移。

| 功能 | 本阶段处理 | 验证状态 |
|---|---|---|
| IPMsg报文、GBK/UTF-8 | 纯Rust解析/编码，边界检查 | 核心单测通过 |
| 用户发现/直接用户 | 真实UDP、BR_ENTRY/ANSENTRY | loopback收发通过；真实飞秋待测 |
| 分组通讯录 | 复用既有React分组与筛选 | 界面待完整构建验证 |
| 文本、多行、字体尾标 | Rust处理、普通文本表情保留 | 核心单测及loopback通过 |
| 消息回执/重发/去重 | 先落库和登记再发送，三次尝试后失败 | 错误来源ACK、重复接收、超时集成测试通过 |
| SQLite历史 | 独立数据库线程、有界队列、分页/搜索/清空 | 数据库测试通过 |
| 配置 | Rust JSON单一来源、校验、临时文件安全替换 | 持久化/失败保留/损坏配置测试通过 |
| 托盘/关闭行为 | Tauri2原生托盘与退出状态机 | 已实现，GUI待测 |
| 并行身份及数据 | 2427默认端口、用户名带rust和端口、独立WebView/数据目录 | 静态核对，桌面多实例待测 |
| 图片/截图/文件传输 | 入口禁用，接收显示不支持，不自动接受 | 不支持图片的无ACK/一次提示测试通过 |
| 原生文件拖放 | 不注册旧事件，不发送文件 | 界面能力禁用 |
| IP范围扫描/自定义网段 | 禁用；保留单个IPv4:port直接用户 | 非本阶段 |
| 提示音/完整系统通知 | 禁用；仅后台消息请求任务栏注意 | 非完整迁移，GUI待测 |
| 旧数据导入 | 不读取旧数据库、IndexedDB、注册表 | 非本阶段 |

## 结构

- `src/`：React前端副本，使用Tauri bridge，无TauriCPP全局或自动成功mock。
- `crates/ipmsg-core/src/protocol.rs`：基础IPMsg编码/解码和IPv4广播计算。
- `crates/ipmsg-core/src/text.rs`：飞秋字体尾标清理。
- `crates/ipmsg-core/src/network.rs`：UDP接收、发现、用户状态、待确认消息、退出取消。
- `crates/ipmsg-core/src/database.rs`：单连接数据库线程及消息存储。
- `crates/ipmsg-core/src/config.rs`：Rust配置校验及持久化。
- `src-tauri/src/commands.rs`：白名单IPC；`runtime.rs`：资源组合/事件转发/退出；`platform.rs`：参数及Windows网卡枚举；`lib.rs`：窗口与托盘。

相比最初目录草案，配置和数据库也归入无Tauri依赖的核心crate，目的是让真实后端逻辑在缺少WebView依赖时仍可离线测试。没有为此引入额外通用抽象层。

## 验证记录

环境：Rust 1.98.1、MSVC target、Node 24.16.0。

已执行：

```text
cargo fmt --manifest-path tauri-rust/Cargo.toml --all
cargo fmt --manifest-path tauri-rust/src-tauri/Cargo.toml --all
cargo test --offline --manifest-path tauri-rust/Cargo.toml -p ipmsg-core
cargo check --offline --manifest-path tauri-rust/Cargo.toml -p ipmsg-core
```

结果：27个单元测试、5个loopback UDP集成测试通过，核心cargo check通过。新增覆盖临时源端口、启动失败可重试、会话过滤后限流、清空的实际ID边界与清空后重传。测试仅使用临时文件和127.0.0.1临时端口，不使用真实聊天数据库或2425/2426生产实例。

前端另对20个TypeScript源文件做语法解析，通过；`node tauri-rust/tools/test-store.cjs frontend/package.json` 借用原工程已安装的TypeScript/Zustand作为测试依赖，验证新store的4组用例：早到ACK、清空期间消息/未读/迟到事件、搜索会话参数、清空失败保留数据。测试运行新目录中的实际store并注入IPC测试替身，不构建GUI，不代表完整类型检查已通过。

受阻检查：

```text
cargo check --offline --manifest-path tauri-rust/src-tauri/Cargo.toml
  no matching package named `wry` found

npm --prefix tauri-rust install --offline --ignore-scripts --no-audit --no-fund
  ENOTCACHED: @tauri-apps/api
```

首次离线检查没有取消offline限制、没有联网补装，所以上述检查当时受阻。用户随后补齐依赖，进入实际构建与启动验证。

## 首次构建与发现/接收修复

1. **Tauri异步命令宏**：带 `State<'_, ...>` 引用参数的async命令必须返回Result，原返回Value触发E0277/E0597。改为 `Result<Value,String>` 并以Ok包装原有业务JSON，保留前端 `{success,error}` 语义。桌面cargo check和前端tsc检查已通过，用户已构建启动GUI。
2. **同名联系人覆盖**：原版和飞秋都可能声明 `feng@FENG`。远端ID改为协议身份+IP+监听端口，回执仍匹配准确来源；临时端口仅在唯一候选时归并。两端点独立发现、同包号独立收信及不串回执的集成测试通过。旧历史不删不自动迁移。
3. **Windows UDP接收循环退出**：向未监听端口发探测后，Winsock会在后续接收返回10054/ConnectionReset；旧分支无条件break，导致只发不收。用与桌面相同的socket2创建方式复现，修复前得到10054并且之后的上线应答超时；改为可恢复错误继续接收，小幅退避，永久错误写入诊断日志。
4. **诊断**：发现TX/RX、解析失败和接收错误通过 `network.diagnostic` 路由至rust-preview.log，不再仅写Release不可见的stderr。`--verbose`可看到可恢复错误和具体端点。

最新核心回归：27个单元测试、8个UDP集成测试通过。测试目录加入进程序号保证并行唯一性，对Windows短暂文件占用进行有界清理重试，持续失败仍使测试失败。修复后的桌面cargo check和前端完整类型检查通过。

最新实机 `rust-preview.log` 已记录 `Discovery RX ... source=192.168.2.88:2425`、`Event user.discovered`、`Event message.ack`、`Event message.received`；日志中再次出现10054后仍继续收到发现应答，确认本次接收循环修复在实际进程中生效。尚未进行完整UI自动化及全部故障场景验收。

## 下一步验收

1. 补齐依赖并运行桌面cargo check及npm run check，修正实际诊断。
2. 启动Rust版2427，与飞秋2425、C++2426分别做真实文本互通；同名协议身份会合并，测试需留意。
3. 验证托盘隐藏、恢复、关闭退出、退出期间收发、同端口重复启动失败提示。
4. 验证窗口刷新/StrictMode不会重复监听或丢首批发现；ACK早于发送返回时仍更新正确消息。
5. 核心验收通过后再单独规划图片/文件/截图，不提前标成支持。

## 后续迁移约束

- 飞秋图片发送必须复用已验证的循环字典LZW及“先图片全部确认、后发引用”顺序，参考父目录 `飞秋扩展协议已验证.md`。
- 文件传输与内嵌图片是不同通道，不能以文件发送假装图片协议。
- 设置保存、历史清空和发送完成均须依据真实后端结果。
- 所有后台任务必须可取消，互斥锁不跨网络/数据库await；正常退出有等待上限。
- 不共享原版数据；迁移旧历史需要未来提供显式导入/备份流程。
