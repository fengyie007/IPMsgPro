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

## 第二阶段2A：图片接收与历史

本轮仅启用收图，不实现图片发送、截图或TCP文件传输。`imageReceive=true`，`imageSend=false`，发送按钮保持禁用。

- 新增纯Rust `image/lzw.rs`、`dib.rs`、`fragments.rs`：循环字典解码、严格DIB/CRC/像素边界校验、原始PNG/JPEG验证、有界分片状态机。
- 接收状态为collecting/finalizing/completed/rejected；以端点ID+wire图片ID隔离，引用先到/图像先到均只形成一条图片历史，最终片不等待引用才ACK。
- `network/image_receive.rs` 使用单个后台处理器解码/落盘，接收主循环不做重型图像处理。冲突、拒收、超时统一发出一次失败通知，不创建假图片消息。
- 图片保存为独立assetId目录中的PNG原图和缩略图，SQLite schema迁移至2，新增image_assets/message_images，原有文本记录及去重记录保留。
- 元数据事务与接收状态通过短提交闸门线性化，数据库真正执行时才验证状态；UDP/计时路径使用try_lock，遇提交占用不阻塞接收、让对端重传。大块文件写入/解码不持此锁。
- 新写资产由PendingAsset守卫持有，守卫随数据库排队任务转移；未提交或异步调用被取消会清理，已提交才保留，避免清理与数据库提交竞争。
- Tauri新增受控 `ipmsg-image` 协议与 `image.read {assetId,thumbnail}`；仅本地主窗口可读取数据库已登记图片，不接受任意磁盘路径，不通过IPC传整图base64。
- 前端实时/历史/搜索保留image元数据；支持缩略图、应用内原图查看、失败重试；同尺寸不同ID不误去重，清空后的迟到事件不复活消息。

验证记录：最终66个Rust单元测试、5项图片接收/迁移集成及8项原有UDP回归通过（共79项）。新增用例覆盖数据库排队期间拒收不得入库、提交等待者取消后文件与数据库一致、阻塞任务结果丢弃时未提交资产清理，以及冲突分片一次性失败通知。前端9组store测试与完整类型检查通过，桌面cargo check通过。另以两个本地真实飞秋LZW样本运行Rust验证程序，200×268和394×198图片的全部像素与发送端BMP一致。未自动启动GUI、打包或进行实际飞秋→Rust界面验收。

### 2A实机兼容补充：飞秋在发送前判定为普通IPMsg

用户反馈飞秋提示“对方正在使用飞鸽，无法接收图片”，此时尚未开始发送图片分片。Rust网络编码仍使用基础版本字段`1`，没有公告飞秋兼容格式，属于2A接入遗漏，而非图片解码失败。

- `protocol.rs` 新增可校验的显式版本编码与飞秋兼容头生成，复用原C++版的 `1_lbt6_0#128#...#0#0#0#4001#9` 模板；基础`encode_packet`仍保留标准IPMsg格式供测试和校验。
- `network.rs` 的发现、应答、文本及ACK统一使用实例兼容头，应用界面版本仍为Rust预览版。模板中的版本值是互通格式，不代表实现了飞秋全部功能，也不添加文件/加密选项位。
- 使用本机协议身份+端口派生稳定的本地管理虚拟MAC形状标识，不复制对端/同机飞秋的MAC；该标识不是认证凭据，不作为Rust联系人键。
- 新增稳定性、实例隔离、报文尾部不变和版本字段注入拒绝测试；UDP测试确认发现与后续文本使用同一扩展头。
- 回归：68个单元测试、13个集成测试通过，桌面cargo check通过；飞秋发送界面的实际放行仍需刷新联系人后验证。

## 下一步验收

1. 用户构建并运行Rust版2427，让飞秋2425和原C++版分别发送图片，确认直接预览、点击原图及重启历史。
2. 检查图片引用不产生额外文本消息，重复发送/重试不重复图片，两个同名不同端点不混图。
3. 验证坏图、磁盘写入失败和退出期间处理的结果；不把收到ACK等同于已渲染。
4. 保持文本收发、搜索、清空、托盘和Windows UDP10054恢复能力不回退。
5. 2A验收通过后再实施2B图片发送，最后接入2C截图；其它按钮不提前解锁。

## 后续迁移约束

- 飞秋图片发送必须复用已验证的循环字典LZW及“先图片全部确认、后发引用”顺序，参考父目录 `飞秋扩展协议已验证.md`。
- 文件传输与内嵌图片是不同通道，不能以文件发送假装图片协议。
- 设置保存、历史清空和发送完成均须依据真实后端结果。
- 所有后台任务必须可取消，互斥锁不跨网络/数据库await；正常退出有等待上限。
- 不共享原版数据；迁移旧历史需要未来提供显式导入/备份流程。
