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

## 注意事项

- 用户仍需在设置中输入**广播地址**（如 `10.8.33.255`），而非 CIDR（`10.8.33.0/24`）。CIDR 转换可作为后续优化。
- 跨网段发现：广播需路由器开启 `ip directed-broadcast`；直接添加用户走单播，不依赖路由器转发。
- 任务栏图标缓存需取消固定重新固定或重启 explorer.exe 刷新。
