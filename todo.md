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
