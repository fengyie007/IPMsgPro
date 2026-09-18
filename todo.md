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
