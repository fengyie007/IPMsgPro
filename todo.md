# Todo

## 修复：跨网段广播发现不工作

**问题**：用户在设置中添加 10.8.33.0/24 网段后，无法扫描到 33 网段的用户。

**根因**：三个 bug 叠加

### 修改 1：`src/bridge/command_handler.cpp` — HandleConfigSet 增加 segments 处理

前端从未将 segments 发送给后端，`HandleConfigSet` 也未处理该字段。

- 在 `HandleConfigSet` 中新增 `segments` 数组字段处理
- 接收前端配置后，清空 `MsgMng::segments_`，再逐条 `AddSegment`
- 状态：✅ 已完成

### 修改 2：`frontend/src/stores/configStore.ts` — 同步 segments 到后端

`loadConfig` 和 `saveConfig` 均未将 segments 同步到 C++ 后端。

- `loadConfig`：启动时将保存的 segments 通过 `invoke('config.set', { segments })` 发送给后端
- `saveConfig`：保存配置时同步 segments 到后端
- 状态：✅ 已完成

### 修改 3：`src/ipmsg/network.cpp` — 恢复定向广播计算

`GetAllBroadcastAddresses()` 被简化为仅返回 `255.255.255.255`，有限广播不会被路由器转发到其他网段。

- 恢复对每个本地网卡调用 `GetBroadcastAddress()` 计算定向广播地址（如 10.8.34.11 → 10.8.34.255）
- 同时移除 `MsgMng::Init` 中将自动检测地址加入 `segments_` 的逻辑，避免与 `UdpBroadcast()` 中直接调用 `GetAllBroadcastAddresses()` 重复发送
- 添加 `#include <algorithm>` 以支持 `std::find` 去重
- 状态：✅ 已完成

### 注意事项

- 用户仍需在设置中输入**广播地址**（如 `10.8.33.255`），而非 CIDR（`10.8.33.0/24`）。CIDR 转换可作为后续优化。

---

## 改进：build.ps1 自动检测 VS 版本

**改动**：`build.ps1` 不再硬编码 `Visual Studio 17 2022`，通过 `vswhere.exe`（VS2017+ 自带）自动检测已安装的 VS 版本。

- 使用 `catalog_productLineVersion` 获取年份（如 `2022`）
- 通过查找表映射到 CMake generator 主版本号（`2022→17`, `2025→18`, `2026→19`）
- 找不到 vswhere 时回退到 VS2022
- 状态：✅ 已完成

---

## 功能：跨网段用户持久化（Settings UI + 启动自动添加）

**需求**：`--adduser` CLI 参数重启后丢失，需在 Settings 持久化配置。

### 修改 4：前端类型与存储
- `frontend/src/types/index.ts`：`Config` 增加 `directUsers: string[]` 字段，`DEFAULT_CONFIG` 初始化为空数组
- `frontend/src/stores/configStore.ts`：`loadConfig`/`saveConfig` 同步 `directUsers` 到后端

### 修改 5：Settings UI
- `frontend/src/components/Settings.tsx`：新增「跨网段直接添加用户」区块
  - 列表显示已添加用户（IP:端口），可删除
  - 输入框支持 `IP:端口` 格式（如 `10.8.33.50:2425`），回车或点击 + 添加
  - 图标：`FiUserPlus` / `FiUsers`

### 修改 6：后端配置处理
- `src/bridge/command_handler.cpp`：`HandleConfigSet` 处理 `directUsers` 数组，解析 `IP:端口` 调用 `MsgMng::AddDirectUser`

### 修改 7：MsgMng 持久化与启动发送
- `src/ipmsg/msgmng.h/.cpp`：
  - 新增成员 `directUsers_` 存储 `(ip, port)` 对
  - `AddDirectUser(ip, port)`：去重添加
  - `GetDirectUsers()`：返回列表供启动使用
  - `src/main.cpp` OnSetup 中遍历 `GetDirectUsers()` 调用 `SendDirectEntry`，实现启动时自动发送 BR_ENTRY

### 使用方式
1. 打开设置 → 跨网段直接添加用户
2. 输入 `10.8.33.50:2425`（目标 IP + 端口）
3. 点击保存
4. 重启程序，自动向该 IP 发送 BR_ENTRY，实现跨网段发现

---

## 修复：直接添加用户重启后丢失 / 刚添加不可见

**问题**：`--adduser` CLI 参数仅启动时生效，重启丢失；Settings 添加后需重启才可见。

### 修改 8：前端配置加载后通知后端
- `frontend/src/App.tsx`：`loadConfig()` 完成后调用 `invoke('config.loaded')`
- `src/bridge/command_handler.h/.cpp`：新增 `HandleConfigLoaded` 命令
- `HandleConfigLoaded` 遍历 `GetDirectUsers()` 发送 BR_ENTRY，无需重启即可见

### 修改 9：配置更新时清理旧 directUsers
- `command_handler.cpp` `HandleConfigSet`：处理 `directUsers` 前先调用 `ClearDirectUsers()`
- `msgmng.cpp` 新增 `ClearDirectUsers()` 清空向量

---

## 修复：聊天记录目录修改不生效

**问题**：设置中修改「聊天记录存储目录」后，数据库仍在默认位置。

### 修改 10：`HandleConfigSet` 重新初始化数据库
- `command_handler.cpp`：`dataDir` 变更时，关闭旧 DB，用新路径 `dataDir + "\ipmsg.db"` 重新 `Init()`
- `dataDir` 清空（恢复默认）时同理
- 状态：✅ 已完成

---

## 改进：重命名应用为「迅秋 (SpeedIPMsg)」

### 修改 11：全代码库替换品牌名
| 文件 | 修改内容 |
|------|----------|
| `src/main.cpp` | 窗口标题 `"倍信"` → `"迅秋"`，托盘提示同理 |
| `frontend/index.html` | `<title>IPMsg Pro - 飞鸽传书</title>` → `"迅秋 (SpeedIPMsg)"` |
| `resources/app.rc` | `FileDescription` / `ProductName` 更新 |
| `frontend/src/components/LeftSidebar.tsx` | 底部版权文本更新 |
| `frontend/src/components/Settings.tsx` | 关于版本显示更新 |
| `README.md` | 标题更新 |
| `frontend/src/types/index.ts` | 注释更新 |
| `frontend/package.json` | `"name": "ipmsgpro"` → `"speedipmsg"` |
| `configStore.ts` / `Settings.tsx` | 默认数据目录 `~/.ipmsgpro` → `~/.speedipmsg` |
| `src/main.cpp` `GetAppDataDir` | 默认目录 `.ipmsgpro` → `.speedipmsg` |
| `command_handler.cpp` `GetDataDir` | 同上 |
| `TauriCPP/src/bridge.cpp` | 同上 |

---

## 改进：build.ps1 支持 VS2026 Build Tools

**背景**：CMake 4.1.2 不支持 VS2026 generator，但机器安装了 VS Build Tools v18 (VS2026)。
- 检测到 VS 18 目录时，使用其自带 `cmake.exe` (4.3.1) 与 `Visual Studio 18 2026` generator
- 生成目录清理避免 generator 冲突
- 状态：✅ 已完成

---

## 修复：分组设置重启后丢失

**问题**：设置中配置「分组」后重启丢失。

**根因**：`configDB.ts` `loadConfig()` 缺少 `group` 字段加载。

### 修改 12：`configDB.ts` 加载 group 字段
- `frontend/src/services/configDB.ts` `loadConfig()` 增加 `group` 字段从 IndexedDB 读取
- 状态：✅ 已完成

---

## 修复：启动时对话列表不显示历史聊天

**问题**：启动时对话列表为空，不显示历史聊天对象。

**根因**：对话列表仅显示 `userStore` 中在线用户，历史记录中的离线用户被忽略。

### 修改 13：对话列表显示所有有消息的用户
- `UserListPanel.tsx`：遍历 `messageStore` 中所有有消息的 partner，创建虚拟 User 对象补充到对话列表
- `loadRecentConversations` 运行顺序调整：在 `loadLocalUserId` 之后执行（确保 localUserId 就绪）
- 移除 7 天过滤，显示所有有历史记录的对话
- 状态：✅ 已完成

---

## 功能：IP 范围扫描（跨网段主动发现）

**需求**：路由器不支持跨网段广播，需支持在 Settings 中配置 IP 范围（如 `10.8.33.1-254`），启动时自动扫描。

### 修改 14：前端类型与存储
- `frontend/src/types/index.ts`：`Config` 增加 `ipScanRanges: string[]` 字段，`DEFAULT_CONFIG` 初始化为空数组
- `frontend/src/services/configDB.ts`：`loadConfig`/`saveConfig` 增加 `ipScanRanges` 读取/保存
- `frontend/src/stores/configStore.ts`：`loadConfig`/`saveConfig` 同步 `ipScanRanges` 到后端

### 修改 15：Settings UI
- `frontend/src/components/Settings.tsx`：新增「IP 范围扫描 (跨网段主动发现)」区块
  - 列表显示已添加范围（格式 `起始IP-结束IP`，如 `10.8.33.1-254`），可删除
  - 输入框支持 `起始IP-结束IP` 格式（支持完整 IP `10.8.33.1-10.8.33.254` 和简写 `10.8.33.1-254`），回车或点击 + 添加
  - 端口、延迟(ms) 可配置
  - 扫描时显示进度条、已发现数量

### 修改 16：后端配置处理
- `src/bridge/command_handler.cpp`：
  - `HandleConfigSet` 处理 `ipScanRanges` 数组，调用 `MsgMng::AddScanRange`/`ClearScanRanges`
  - `HandleConfigLoaded` 启动时自动调用 `ScanIpRanges`
  - `HandleNetworkScanRange` 支持 `ranges` 数组格式（兼容旧 `startIp/endIp`）

### 修改 17：MsgMng IP 范围扫描核心实现
- `src/ipmsg/msgmng.h/.cpp`：
  - 新增 `scanRanges_` 存储范围字符串
  - `AddScanRange`/`ClearScanRanges`/`GetScanRanges`
  - `ScanIpRanges(ranges, port, delayMs)`：支持多范围顺序扫描，逐个 IP 发送 BR_ENTRY
  - 扫描后 3 秒宽限期等待异步响应
  - 扫描进度回调（每 10 个 IP 触发）、完成回调

### 修改 18：前端自动触发
- `frontend/src/App.tsx`：启动时检查 `ipScanRanges`，自动调用 `invoke('network.scan_range')`
- `Settings.tsx`：保存时同步 `ipScanRanges`，扫描按钮调用 `network.scan_range` 传递 `ranges` 数组

### 修改 19：简写 IP 范围支持
- 支持完整 IP 格式：`10.8.33.1-10.8.33.254`
- 支持简写格式：`10.8.33.1-254`（自动补全前三段）

### 修改 20：扫描日志增强
- 扫描开始/结束、每个 IP 发送 BR_ENTRY 详细日志
- 发现新用户时记录日志
- 进度、完成日志包含发现数量

---

## 修复：IP 范围扫描线程未回收导致崩溃

**问题**：配置了 IP 段的会话，扫描过一次之后再次点击「开始扫描」、或正常退出程序时直接崩溃（日志出现 `CRASH` / `std::terminate`）。

**根因**：`ScanIpRanges` 直接给 `scanThread_` 赋新线程，而上一次自然结束的扫描线程仍处于 joinable 状态；`Shutdown` 也从不 join 扫描线程。C++ 规定给 joinable 的 `std::thread` 赋值或析构会调用 `std::terminate`。

### 修改 21：`src/ipmsg/msgmng.cpp` — 扫描线程生命周期
- `ScanIpRanges`：占用扫描标志后先 join 上一个扫描线程，再创建新线程
- `CancelScan`：无论扫描是否仍在进行都 join，保证 `scanThread_` 不残留 joinable
- `Shutdown`：先停止并回收扫描线程，再关闭 socket（放在 `ready_` 判断之前，因为 Init 失败时也可能已启动扫描）
- 顺带修正扫描日志里多调用一次 `MakePacketNo()` 的问题（日志打印的包号与实际发送的不一致，且白白消耗包号）
- 状态：✅ 已完成

### 修改 22：`src/bridge/command_handler.cpp` / `frontend/src/App.tsx` — 去掉启动时的重复扫描触发
- 启动扫描只保留后端 `HandleConfigLoaded` 一处，删除 `App.tsx` 中 `config.loaded` 之后再次调用 `network.scan_range` 的代码
- `HandleConfigLoaded` / `HandleNetworkScanRange` 的目标端口不再硬编码 2425，改用 `GetLocalPort()`
- 扫描进度/完成回调改为在 `SetupEventForwarding` 中一次性注册：后端自动触发的扫描也能向前端发事件，同时消除 UI 线程反复替换回调与扫描线程读取回调之间的竞态
- 状态：✅ 已完成

---

## 修复：聊天记录超过 50 条后只显示最旧的 50 条

**问题**：与某人的消息超过 50 条后，打开会话看到的是最早的 50 条，最新的消息看不到。

**根因**：`MessageDB::GetMessages` 的 SQL 先按时间升序再 `LIMIT 50`，取到的是最旧的一页；前端 `loadHistory` 默认 `limit=50, offset=0`。

### 修改 23：`src/database/message_db.cpp` — 取最新 N 条再按时间升序返回
- 内层子查询按 `timestamp DESC, rowid DESC` 取 `LIMIT ? OFFSET ?`（offset 语义变为向更早的历史翻页），外层再按 `timestamp ASC, rowid ASC` 排序供聊天面板显示
- 以 `rowid` 作为同一秒内多条消息的次序，避免分页时跳过或重复
- `message_db.h` 注释同步更新；前端调用方式不变
- 状态：✅ 已完成

---

## 修复：昵称与分组互相覆盖

**问题**：同时设置了昵称和分组后，对端（飞秋 / 第二实例）看到的昵称变成登录名，或分组为空；重启后现象不定。

**根因**：前端 `configStore` 把 `nickname` 和 `group` 拆成两次 `config.set` 发送，而 `MsgMng::UpdateLocalInfo` 每次都无条件覆盖两个字段，后发的一次把另一个清空。

### 修改 24：`src/ipmsg/msgmng.{h,cpp}` / `src/bridge/command_handler.cpp` — 只更新本次传入的字段
- `UpdateLocalInfo` 参数改为 `std::optional<std::string>`，只在有值时更新对应字段；昵称为空时回退为登录名（与 `Init` 一致）；仅在有变化时重新广播 BR_ENTRY
- `HandleConfigSet` 只在请求里存在 `nickname` / `group` 键时才传值
- 状态：✅ 已完成

---

## 修复：消息与文件状态从不写回数据库，历史文件永远显示 0%

**问题**：重启后，历史记录里发出的文件一直显示 0% 进度条，收到的文件没有「已保存 / 打开文件夹」；文本消息的送达状态不保存。

**根因**：`MessageDB` 只有 `INSERT`，没有任何 `UPDATE`；后端发出的 `message.ack` 事件前端无人监听，且事件里带的是回执自身的包号而非被确认的包号；前端 `loadHistory` 把状态 <2 的文件一律伪造成 0% 或「等待接收」。

### 修改 25：`src/database/message_db.{h,cpp}` — 状态枚举与 `UpdateStatus`
- 新增 `MessageStatus` 枚举：0 Sending、1 Delivered、2 Completed（文件传完）、3 Failed
- 新增 `UpdateStatus(id, status)`（`UPDATE messages SET status=? WHERE id=?`）

### 修改 26：`src/bridge/command_handler.{h,cpp}` / `src/ipmsg/msgmng.{h,cpp}` — 写回状态
- `MsgMng::SendMessage` 改为返回本次发送的 packetNo（0 表示失败）
- 新增 `pendingAcks_`（packetNo → 消息 id，带互斥锁与 10 分钟过期清理）：发送文本时登记，收到 RECVMSG 时按**回执正文里的包号**取回并写入 Delivered；`message.ack` 事件增加 `messageId` 字段
- 文件传输进度回调：Completed → 写 2，Failed → 写 3（文件记录的 id 就是 transferId，收发两端一致）
- 各处裸状态码替换为枚举常量；发送失败写 3（原先写 2，与「已完成」冲突）

### 修改 27：`frontend/src/stores/messageStore.ts` — 历史状态映射与回执监听
- 抽出 `historyState()`：文件 2 → 100% 已完成，3 → 失败，0/1 → 普通卡片（不再伪造 0% 或「等待接收」）；文本 0 → 发送中，1 → 已送达，其他 → 失败
- 新增 `message.ack` 监听：按 `messageId` 把自己发出的文本消息置为 delivered
- 状态：✅ 已完成

---

## 修复：接收文件名未消毒且同名文件被静默覆盖

**问题**：对端发来的文件名直接拼进 `Downloads\` 路径，带 `..\` 可写到目录之外；同名文件直接被截断覆盖。

### 修改 28：`src/bridge/command_handler.cpp` — `SanitizeFileName` / `UniqueSavePath`
- 新增 `SanitizeFileName`：只保留最后一级文件名，替换 Windows 非法字符与控制字符，去掉末尾的点和空格，规避 CON/NUL/COM1 等保留设备名；`HandleFileSaveTemp` 原有的内联消毒逻辑改为复用它
- 新增 `UniqueSavePath`：目标已存在时依次尝试 `名字 (1).ext`、`名字 (2).ext`
- `HandleFileAccept` 自动生成保存路径时同时使用二者；数据库与 `file.transfer_started` 事件中的路径即最终落盘路径
- 状态：✅ 已完成

---

## 修复：跨线程共享状态无保护（数据库热切换、本地用户信息、传输线程生命周期）

**问题**：修改数据目录时 UI 线程关闭并重开 SQLite，接收线程可能正在写入；昵称/分组由 UI 线程改写而接收线程、扫描线程同时读取；退出时文件传输对象被删除，而 detached 的收发线程可能仍在运行。

### 修改 29：`src/database/message_db.{h,cpp}` — 所有公开方法加锁
- 新增 `mutex_`，`Init`/`Close`/`SaveMessage`/`UpdateStatus`/查询方法全部在锁内执行；`Init` 内部用 `CloseLocked` 原子地完成关旧开新
- `HandleConfigSet` 不再先 `Close()` 再 `Init()`，避免中间窗口丢消息

### 修改 30：`src/ipmsg/msgmng.{h,cpp}` — 本地用户信息快照
- 新增 `localUserMutex_` 与 `LocalUserSnapshot()`；`MakeMsg`、`ProcessRecvBuffer`、广播函数、扫描线程一律读快照，`UpdateLocalInfo` 在锁内写
- `GetLocalUser()` 改为返回副本；`ready_` 改为 `std::atomic<bool>`
- `segments_`/`directUsers_`/`scanRanges_` 仅在 UI 线程读写，加注释说明

### 修改 31：`src/file/file_transfer.{h,cpp}` — 工作线程可回收
- 新增 `activeWorkers_` 计数（RAII 递减），`Shutdown` 取消所有传输后最多等待 5 秒让 detached 线程退出，再返回给 `main` 删除对象
- accept 到的 socket 设置收发超时（10s/30s），避免对端停滞把线程钉死
- 收发循环同时检查 `running_`，`Shutdown` 后立即退出
- `UpdateTransferProgress` 在锁外调用进度回调，消除回调内再查询本对象的死锁隐患；`ready_` 改为原子
- 状态：✅ 已完成

---

## 修复：不同发送方的消息主键可能碰撞导致历史丢失

**问题**：两个对端在同一秒发来的消息，或同一对端重启后重复使用的包号，会在数据库中得到相同的 id，`INSERT OR IGNORE` 静默丢弃后一条。

**根因**：收到的消息直接以对端的 packetNo 作主键，而 packetNo 只在单个发送方内唯一（各自从启动时间开始计数）。

### 修改 32：`src/bridge/command_handler.cpp` — 收到的消息 id 改为 `发送方Key:packetNo`
- `message.received` 事件与数据库记录使用同一个复合 id，实时消息与历史记录保持一致
- 文件接收请求里的 `transferId` 仍为裸 packetNo（`file.reject` 需要解析它来发送 RELEASEFILES），不受影响；旧数据保留原 id
- 状态：✅ 已完成

---

## 修复：路径处理混用 ANSI 与 UTF-8，中文用户名 / 中文目录下数据库、日志、临时文件失效

**问题**：自定义数据目录经 `CreateDirectoryA` / `RegSetValueExA` 写入 UTF-8 字节，含中文时生成乱码目录，下次启动从注册表读回的路径也对不上；默认数据目录用 `GetEnvironmentVariableA` 得到 ANSI 字节，却直接交给只认 UTF-8 的 `sqlite3_open`，中文用户名的机器上历史数据库打不开；日志文件、截图临时文件、`file.save_data` 均用窄字符路径打开；前端注入的 `defaultDataDir` 也是 ANSI 字节。

### 修改 33：新增 `src/util/encoding.{h,cpp}` 与 `src/util/app_paths.{h,cpp}`
- `enc::Utf8ToWide` / `WideToUtf8` / `Utf8ToAnsi` / `AnsiToUtf8` / `IsValidUtf8` / `EnsureUtf8` / `PathFromUtf8` / `GetEnvUtf8`：全后端唯一定义（各文件里的重复静态副本留待后续清理）
- `paths::DefaultDataDir` / `ReadCustomDataDir` / `WriteCustomDataDir`（注册表 `*W` 版）/ `ApplyPortSuffix` / `ResolveDataDir` / `UserDownloadsDir` / `AppTempDir`：所有返回值均为 UTF-8
- `CMakeLists.txt` 加入两个新源文件

### 修改 34：调用方切换到宽字符 API
- `src/main.cpp`：删除本地 `GetAppDataDir` / `Utf8ToWide` / `WideToUtf8`，启动数据目录改用 `paths::ResolveDataDir(port)`
- `src/logger.cpp`：日志文件通过 `enc::PathFromUtf8` 打开（Init / Reinit / 延迟打开三处）
- `src/bridge/command_handler.cpp`：`HandleConfigSet` 用 `fs::create_directories(宽路径)` + `paths::WriteCustomDataDir`；`GetDataDir()` 与启动规则一致（含端口后缀，修正了运行时切换目录后与启动时目录不一致的问题）；`HandleFileSaveTemp` 临时目录改用 `paths::AppTempDir()` 并以宽路径写入；`HandleFileSaveData` 以宽路径创建目录和文件；`GetUserDownloadsDir` 改为转调 `paths::UserDownloadsDir`
- `src/file/file_transfer.cpp`：失败后删除半截文件改用 `PathFromUtf8`
- `TauriCPP/src/bridge.cpp`：注入前端的 `homeDir` / `defaultDataDir` 改由 `GetEnvironmentVariableW` + UTF-8 转换得到
- 状态：✅ 已完成

---

## 修复：几处会静默失效或直接抛异常的健壮性问题

### 修改 35：`src/ipmsg/network.{h,cpp}` — 定向广播使用真实子网掩码
- 原先 `GetBroadcastAddress` 硬编码 /24，/16、/22 等网段的定向广播地址算错，导致跨交换机的同网段用户发现不到
- 新增 `LocalAddress{ip, prefixLength}` 与 `GetLocalAddresses()`（取 `GetAdaptersAddresses` 的 `OnLinkPrefixLength`），`GetBroadcastAddress(ip, prefixLength)` 按掩码计算；/31、/32 点对点链路不产生定向广播

### 修改 36：`src/file/file_transfer.{h,cpp}` — 去掉 TCP 端口 +1/+2 回退
- 对端只会连接我们 UDP 包的源端口，回退到 2426/2427 后所有发出的文件都会静默失败；改为绑定失败直接记录 ERROR 并禁用文件传输

### 修改 37：`src/bridge/command_handler.cpp` / `src/main.cpp` — 解析 `IP:端口` 时不再抛异常
- `HandleConfigSet` 处理 `directUsers`、`WinMain` 处理 `--adduser` 时，端口非法（非数字、超出 1~65535）只跳过该条并记日志，原先 `std::stoi` 抛出后整个 `config.set` 失败或启动前崩溃
- 状态：✅ 已完成

---

## 修复：日志洪水阻塞接收线程并撑爆日志文件

**问题**：每个 UDP 包都被逐 16 字节十六进制转储并逐行刷盘，代码里自己的注释已经说明这会阻塞接收线程、导致飞秋大截图的分片丢失；文件传输每 64KB 一行、每次广播都枚举一遍网卡、每条消息约 12 行摘要；飞秋调试转储无条件写进用户的 Downloads。日志没有级别概念，约 240 处调用传空级别。

### 修改 38：`src/logger.{h,cpp}` — 日志级别
- 新增 `LogLevel`（DEBUG < INFO < WARN < ERROR）、`SetLogLevel` / `GetLogLevel` / `IsDebugEnabled`
- `LogMessage` 在取锁和格式化之前按级别过滤；空级别字符串视为 INFO，`CRASH` 视为 ERROR
- 默认阈值 INFO；`--verbose` 或 `--log-level=debug` 启动参数切到 DEBUG（`main.cpp` 解析并在日志头部记录当前级别）

### 修改 39：高频日志降为 DEBUG，昂贵的转储用 `IsDebugEnabled()` 包裹
- `msgmng.cpp`：每包十六进制转储、协议字段解析、每个广播地址、每个扫描 IP、发文件原始报文
- `file_transfer.cpp`：TCP 请求转储、GETFILEDATA 原始字节、已登记文件信息、连接/线程生命周期细节；接收进度改为每 1MB 一行（与发送侧一致）
- `network.cpp`：每次广播触发的网卡枚举明细
- `command_handler.cpp`：每条消息的 12 行摘要、进度回调、附件 extra 转储、对话框与接收流程细节；真正的错误改为 ERROR 级别

### 修改 40：`src/bridge/command_handler.{h,cpp}` — 飞秋调试转储改为 DEBUG 且写入数据目录
- 新增 `DumpDebugFile(name, data)`：仅在 DEBUG 级别下写到 `<数据目录>\debug\`，六处 `FeiQ_*.bin` 不再无条件写入用户的 Downloads
- 状态：✅ 已完成

---

## 改进：编码转换收敛到 `util/encoding`，删除四份重复实现

**问题**：UTF-8/GBK/宽字符互转在 `main.cpp`、`msgmng.cpp`、`file_transfer.cpp`、`command_handler.cpp` 各有一份静态副本，细节（是否去掉尾部 NUL、ASCII 快速路径、失败时返回值）互不一致；`network.cpp`、`HandleDialogSave` 还各自手写了一遍 `WideCharToMultiByte`。

### 修改 41：全部调用点改用 `enc::*`
- `msgmng.cpp` / `command_handler.cpp` 保留原有短名（`GBKToUTF8`、`Utf8ToGbk` 等）作为一行内联转发，协议代码不必改动；`file_transfer.cpp` 直接 `using enc::PathFromUtf8`
- `network.cpp` 的 `GetHostName` / `GetUserName`、`HandleDialogSave` 的路径回传改用 `enc::WideToUtf8`
- 提示音临时文件改用 `paths::AppTempDir()` 与宽字符 `mciSendStringW`，`%TEMP%` 含中文时也能播放
- 现在整个 `src/` 只有 `util/encoding.cpp` 直接调用 `MultiByteToWideChar` / `WideCharToMultiByte`
- 顺带：`TauriCPP/src/window.cpp` 每 2 秒一次的 `[drop] RegisterNativeDropTargets done` 输出删除（它经 cerr 重定向进应用日志，是日志尾部反复出现的噪音）
- 状态：✅ 已完成

---

## 改进：清理前后端死代码

### 修改 42：后端
- 删除飞秋内联截图**发送**通道（v1.4.0 起截图已改走标准文件传输，前端不再调用）：`feiq.screenshot_send` / `feiq.echo_screenshot` 命令、`HandleFeiQScreenshotSend` / `HandleFeiQEchoScreenshot` / `SendFeiQShotPayload`、`LzwCompress`、`PngDataUrlTo24bppDib`、`JpegDimensions`、GDI+ 编码器查找、`lastFeiQShot*` 成员、`MsgMng::SendRawCommand`；接收/解码路径（`LzwDecompress`、分片重组）完整保留
- 删除前端从不调用的 `message.send_image`（与 `file.send` 重复约 100 行）和 `file.recv`（与 `file.accept` 重复）
- 删除未使用的 `IsInSubnet`、`MessageDB::GetMessageCount`、`FileTransferManager::SendFileRequest`（空桩）与 `FileReceiveRequestCallback`
- `command_handler.cpp` 由 2960 行减至约 2340 行

### 修改 43：前端
- 删除 `messageVersion`（写 6 处从不读）、`isFeiqShot`、`Config.password`、`App.tsx` 中重复的两次取消订阅
- 删除永远走不到的 base64 发文件路径（`sendFile`、`previewFile`、隐藏的 `<input type="file">`、图片模式的预览弹窗）；`SendPreview` 简化为只展示文件名与大小
- `bridge.ts` 的 dev 模式 mock 与实际调用的命令对齐：补齐 `user.local`、`config.loaded`、`network.scan_range/cancel`、`history.get_recent`、`dialog.open/save`、`file.info/save_temp/save_data/open_folder`、`screenshot.capture`、`window.*`、`shell_open`，删除从未调用的条目
- 状态：✅ 已完成

---

## 改进：配置同步收敛为一次 `config.set`

**问题**：`configStore` 启动时最多发 8 次、保存时最多发 8 次 `config.set`，每个字段一段几乎相同的代码；`configDB.loadConfig` 对每个字段手写一遍读取，新增配置项要改三处。

### 修改 44：`frontend/src/stores/configStore.ts`
- 定义 `BACKEND_KEYS`（后端 `HandleConfigSet` 消费的 8 个键；`port`、`autoDiscovery` 仅前端使用）
- `loadConfig` 把持久化配置一次性发给后端（空 `dataDir` 省略，避免后端无谓地重开日志与数据库）；`saveConfig` 只转发本次变更中的后端相关键，同样只发一次
- 后端 `HandleConfigSet` 本就支持一次接收全部键，无需改动

### 修改 45：`frontend/src/services/configDB.ts`
- `loadConfig` 改为按 `DEFAULT_CONFIG` 的键循环读取，并按默认值的类型校验存储值（类型不符时忽略并告警），`minimizeBehavior` 与 `port` 做取值范围校验
- 新增配置项只需在 `types/index.ts` 的 `Config` / `DEFAULT_CONFIG` 中声明，读取与同步自动覆盖
- 状态：✅ 已完成

---

## 改进：收到消息不再抢焦点，改为未读角标；自动滚动只在贴底时触发

**问题**：任何人发来消息都会把当前会话切过去，`ChatPanel` 按用户 id 重建，正在输入的草稿直接丢失；传文件时每个进度事件都把消息列表拽到底部，用户无法翻看历史。

### 修改 46：`frontend/src/stores/messageStore.ts` — 未读计数
- 新增 `unread: Map<userId, number>` 与 `clearUnread(userId)`；`recvMessage` 收到非当前会话的他人消息（含文件接收请求）时计数 +1
- 删除 `message.received` 与 `feiq.screenshot_received` 里自动 `setCurrentUser` 的逻辑（发送方仍会自动加入联系人列表）

### 修改 47：`frontend/src/components/ChatPanel.tsx` — 打开会话清未读、滚动策略
- 打开会话时调用 `clearUnread`
- 自动滚动改为：仅当用户本来就在底部附近（40px 内）或最新一条是自己发的才滚到底；依赖项改为最后一条消息 id + 条数，不再因进度事件或待接收列表变化触发

### 修改 48：`UserListPanel.tsx` / `LeftSidebar.tsx` — 角标
- 会话卡片右下角显示红色未读数（99+ 封顶），有未读时昵称加粗
- 左侧「对话」导航按钮显示未读总数角标
- 状态：✅ 已完成

---

## 修复：IP 范围扫描进度永远显示 0/0，扫描结束后按钮不复位

**问题**：后端进度回调传的 current/total 都是 0，设置页进度条从不出现；`network.scan_complete` 无人处理，「取消扫描」按钮一直停留；「发现」计数跨次扫描累加。

### 修改 49：`src/ipmsg/msgmng.cpp` — 真实进度
- 抽出 `ParseScanRange`，扫描前先解析全部范围并算出总 IP 数，非法范围直接跳过并告警
- 回调携带 `(current, total, found)`，每 10 个 IP 和每段末尾各上报一次；`scanFoundCount_` 每次扫描开始时清零
- 自己的 IP 跳过发送但仍计入进度，避免进度条永远到不了 100%

### 修改 50：`frontend/src/components/Settings.tsx` / `App.tsx`
- 设置页订阅 `network.scan_progress` / `network.scan_complete`：进度条按 current/total 绘制，完成后按钮复位并显示「扫描完成，发现 N 个用户」；后端启动时自动触发的扫描同样可见
- `network.scan_range` 返回失败（已有扫描在进行）时立即复位
- 删除 `App.tsx` 里只打 console 的两个扫描监听
- 状态：✅ 已完成

---

## 改进：设置页整理与输入校验

**问题**：「端口号」出现两次且绑定同一字段，而后端只读启动参数 `--port`，改了不生效；「自动发现」开关没有任何代码读取；三个列表输入不做格式校验，非法值直到后端才被跳过或静默失败。

### 修改 51：`frontend/src/types/index.ts` / `configDB.ts` / `configStore.ts`
- `Config` 删除无效的 `port` 与 `autoDiscovery`（IndexedDB 里的旧值会被 `loadConfig` 忽略）

### 修改 52：新增 `frontend/src/utils/netValidation.ts`
- `normalizeSegment`：接受广播地址或 CIDR（CIDR 自动换算为定向广播地址，与后端期望一致）
- `normalizeDirectUser`：`IP:端口`，端口 1~65535
- `normalizeScanRange`：`起始IP-结束IP` 或 `起始IP-末段`，简写展开为完整形式存储，结束不得小于起始，最多 65536 个地址

### 修改 53：`frontend/src/components/Settings.tsx`
- 「网络设置」改为只读显示本机监听端口（来自 `user.local`）并说明由 `--port` 决定；删除「自动发现」开关与扫描区里重复的端口输入
- 三个列表输入接入校验：非法时红框 + 红字提示且不入列表，输入变化时清除提示；网段配置增加说明文字
- 状态：✅ 已完成

---

## 功能：图片消息显示缩略图，点击打开原图

**问题**：README 承诺的缩略图只对飞秋内联截图生效；走标准文件通道收发的图片（含截图）只显示文件名。

### 修改 54：`src/bridge/command_handler.{h,cpp}` — `file.read_image`
- 读取本地图片（png/jpg/gif/bmp/webp，宽字符路径），限制 5MB，返回 `data:` URL；超限返回 `tooLarge`

### 修改 55：`frontend/src/components/ChatPanel.tsx` — `useThumbnail`
- 图片气泡按本地路径懒加载缩略图：自己发的立即加载，收到的在传输完成后加载（未完成时文件不完整）；模块级缓存（最多 200 条）避免切换会话时重复读取
- 点击缩略图通过 `shell_open` 用系统默认看图程序打开原图；读取失败或超限时回退为文件名
- `bridge.ts` 补 `file.read_image` mock
- 状态：✅ 已完成

---

## 改进：应用内提示替代原生弹窗；文本消息送达状态；日期分隔；加载更早消息

### 修改 56：新增 `stores/toastStore.ts` + `components/Toast.tsx` + `components/ConfirmDialog.tsx`
- 轻量 toast（info / success / error，自动消失，可手动关闭），挂在 `App` 根部
- 应用内确认对话框（Esc 取消、Enter 确认），替代 `window.confirm`
- `ChatPanel` 的截图失败、打开文件夹回退、文本发送失败，`ScreenshotEditor` 的保存结果，全部改为 toast；「清空聊天记录」改用确认对话框

### 修改 57：`ChatPanel.tsx` — 文本消息状态与日期分隔
- 自己发出的文本消息在时间旁显示状态图标：时钟（已发送等待确认）、绿勾（对方已收到，依赖 `message.ack`）、红叹号（失败）
- 相邻两条消息跨天时插入「今天 / 昨天 / 9月18日 周四」分隔条

### 修改 58：`messageStore.ts` / `ChatPanel.tsx` — 加载更早的消息
- 新增 `historyPages`（每个会话的 offset 与 hasMore）与 `loadMoreHistory`，利用后端已支持的 `offset` 向更早翻页（每页 50 条）
- 消息列表顶部出现「加载更早的消息」按钮；加载后按新增高度补偿滚动位置，视图停留在原来的消息上
- 状态：✅ 已完成

---

## 改进：视觉一致性

### 修改 59：使用 Tailwind 主题 token，替换硬编码色值与宽度
- `tailwind.config.js` 补充 `list.bg`；`LeftSidebar` / `UserListPanel` / `ChatPanel` 中的 `bg-[#2C2C2C]`、`bg-[#3C3C3C]`、`bg-[#F7F7F7]`、`bg-[#F5F5F5]`、`w-[60px]`、`w-[300px]` 改为 `bg-sidebar-bg`、`bg-sidebar-hover`、`bg-list-bg`、`bg-chat-bg`、`w-sidebar`、`w-user-list`，以后改主题只需改配置

### 修改 60：左侧栏 Logo 使用应用图标
- 由 `resources/icon.ico` 生成 `frontend/src/assets/app-icon.png`（64px），替换占位字母「P」，与 exe / 托盘图标一致
- 状态：✅ 已完成

---
