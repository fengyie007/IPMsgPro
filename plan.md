## 产品概述

参考TauriCPP框架重新实现IPMsg（飞鸽传书）应用，前端使用React构建类微信风格的三栏布局界面，后端移植ipmsg-master核心网络协议，实现完全兼容IPMsg v3.0协议（UDP 2425端口）的即时通讯应用，新代码放到src中，参考项目不要动。

## 核心功能

### 1. 用户管理功能

- 自动发现局域网用户（UDP广播）
- 支持多网段设置（默认当前局域网，可添加多个网段）
- 用户名设置
- 用户状态管理（在线/离开/离线）

### 2. 消息传输功能

- 文字消息发送和接收
- 图片发送和预览
- 文件传输（支持大文件）
- 文件夹传输

### 3. 界面布局（类微信风格）

- **左侧工具栏**：Logo、对话列表、设置按钮
- **中间用户列表**：搜索框、刷新按钮、用户卡片列表
- **右侧对话框**：选中用户的消息列表、输入区域（支持文字、图片、文件）

### 4. 设置功能

- 网段配置（多网段管理）
- 用户名和昵称设置
- 端口号设置（默认2425）
- 其他配置选项

### 5. 协议兼容性

- 完全兼容IPMsg v3.0协议
- 可与原始IPMsg客户端互相通信
- 支持UDP广播/单播消息
- 支持TCP文件传输

## 技术栈选择

### 后端技术栈

- **核心框架**: C++17 + TauriCPP
- **网络库**: 移植ipmsg-master的MsgMng类（Winsock2）
- **协议**: IPMsg v3.0（UDP 2425端口）
- **JSON库**: nlohmann/json（已集成）
- **构建系统**: CMake

### 前端技术栈

- **框架**: React 18 + TypeScript
- **构建工具**: Vite
- **样式方案**: Tailwind CSS
- **状态管理**: Zustand
- **图标库**: React Icons
- **配置存储**: IndexedDB（前端本地存储配置）
- **历史消息存储**: SQLite3（通过C++后端管理）

## 实现方案

### 1. 系统架构设计

采用前后端分离架构，通过TauriCPP的Bridge实现C++后端与React前端的双向通信。

**架构层次**：

```
┌─────────────────────────────────────────────────┐
│          React前端 (WebView2)                    │
│  ┌──────────┬──────────┬──────────┐            │
│  │左侧工具栏 │ 用户列表  │ 对话框   │            │
│  └──────────┴──────────┴──────────┘            │
│  ↑↓ Bridge通信 (JSON)                          │
├─────────────────────────────────────────────────┤
│       C++后端 (TauriCPP)                        │
│  ┌──────────┬──────────┬──────────┐            │
│  │命令处理器 │ 消息管理器│ 配置管理 │            │
│  └──────────┴──────────┴──────────┘            │
│  ↑↓ Winsock2 (UDP/TCP)                         │
├─────────────────────────────────────────────────┤
│          网络层 (IPMsg协议)                      │
│  ┌────────────────┬────────────────┐            │
│  │ UDP 2425广播   │ TCP文件传输    │            │
│  └────────────────┴────────────────┘            │
└─────────────────────────────────────────────────┘
```

### 2. 后端实现策略

#### 2.1 移植IPMsg核心网络代码

基于`ipmsg-master/src/msgmng.cpp/.h`移植以下核心功能：

- **UDP通信**: `UdpSend/UdpRecv` - 消息广播和接收
- **TCP通信**: `Accept/Connect/SendFile/RecvFile` - 文件传输
- **用户发现**: `IPMSG_BR_ENTRY/EXIT`广播处理
- **消息封装**: `MakeMsg/ResolveMsg` - 协议消息编解码

**关键修改点**：

- 移除Windows GUI依赖（HWND、消息循环等）
- 改为回调函数/事件驱动模式
- 适配TauriCPP的Bridge通信机制

#### 2.2 Bridge命令设计

创建以下Bridge命令供前端调用：

| 命令类别 | 命令名称 | 功能描述 | 参数 |
| --- | --- | --- | --- |
| 用户管理 | `user.discover` | 发现局域网用户 | `{segments: string[]}` |
| 用户管理 | `user.list` | 获取在线用户列表 | - |
| 用户管理 | `user.status` | 更新用户状态 | `{status: string}` |
| 消息传输 | `message.send` | 发送文字消息 | `{target: string, content: string}` |
| 消息传输 | `message.send_image` | 发送图片 | `{target: string, image: string}` |
| 文件传输 | `file.send` | 发送文件 | `{target: string, filepath: string}` |
| 文件传输 | `file.recv` | 接收文件 | `{transferId: string, savepath: string}` |
| 历史消息 | `history.get` | 获取历史消息 | `{userId: string, limit: number, offset: number}` |
| 历史消息 | `history.search` | 搜索历史消息 | `{keyword: string, userId?: string}` |
| 历史消息 | `history.clear` | 清空历史消息 | `{userId?: string}` |
| 网络管理 | `network.scan` | 扫描指定网段 | `{segment: string}` |


**配置管理（IndexedDB）**：

- 配置直接存储在前端IndexedDB中，无需通过Bridge命令
- 前端实现`configDB.ts`服务封装IndexedDB操作
- 配置包括：用户名、网段列表、端口号、自动发现开关等

**事件推送**（C++ → React）：

- `user.discovered` - 发现新用户
- `user.status_changed` - 用户状态变化
- `message.received` - 收到新消息
- `file.transfer_progress` - 文件传输进度

#### 2.3 多网段支持实现

修改`MsgMng`类的广播逻辑，支持多个网段：

```cpp
class MultiSegmentManager {
    std::vector<std::string> segments_;  // 网段列表
    std::vector<SOCKET> udp_sockets_;     // 每个网段一个UDP socket
    
    bool AddSegment(const std::string& segment);
    bool RemoveSegment(const std::string& segment);
    bool BroadcastToAll(const char* msg);  // 向所有网段广播
};
```

### 3. 前端实现策略

#### 3.1 三栏布局组件设计

**组件树结构**：

```
App.tsx
├── LeftSidebar.tsx          // 左侧工具栏
│   ├── Logo区域
│   ├── 导航按钮（对话、通讯录、设置）
│   └── 用户头像
├── UserListPanel.tsx        // 中间用户列表
│   ├── SearchBar.tsx        // 搜索框
│   ├── RefreshButton.tsx    // 刷新按钮
│   └── UserCard.tsx         // 用户卡片
└── ChatPanel.tsx            // 右侧对话框
    ├── ChatHeader.tsx       // 选中用户信息
    ├── MessageList.tsx      // 消息列表
    │   └── MessageBubble.tsx // 消息气泡
    └── MessageInput.tsx     // 输入区域
        ├── TextInput.tsx
        ├── ImageUpload.tsx
        └── FileUpload.tsx
```

#### 3.2 状态管理设计

使用Zustand管理应用状态：

```typescript
// userStore.ts - 用户状态
interface UserStore {
    users: User[];
    currentUser: User | null;
    loading: boolean;
    discoverUsers: () => Promise<void>;
    setCurrentUser: (user: User) => void;
}

// messageStore.ts - 消息状态（从SQLite3加载历史消息）
interface MessageStore {
    messages: Map<string, Message[]>;
    sendMessage: (target: string, content: string) => Promise<void>;
    recvMessage: (message: Message) => void;
    loadHistory: (userId: string, limit: number) => Promise<void>;
}

// configStore.ts - 配置状态（使用IndexedDB存储）
interface ConfigStore {
    config: Config;
    loadConfig: () => Promise<void>;
    saveConfig: (config: Partial<Config>) => Promise<void>;
}
```

#### 3.3 数据存储策略

**配置存储（IndexedDB）**：

- 使用IndexedDB存储用户配置（用户名、网段、端口等）
- 前端直接读写，无需通过Bridge调用C++后端
- 实现配置服务层封装IndexedDB操作

```typescript
// services/configDB.ts - IndexedDB配置存储服务
class ConfigDB {
    private dbName = 'ipmsg-config';
    private version = 1;
    
    async get(key: string): Promise<any>;
    async set(key: string, value: any): Promise<void>;
    async clear(): Promise<void>;
}
```

**历史消息存储（SQLite3）**：

- C++后端使用SQLite3存储所有消息历史
- 前端通过Bridge命令查询历史消息
- 支持按用户、时间范围查询
- 支持消息搜索

```cpp
// src/database/message_db.cpp - 消息数据库管理
class MessageDB {
public:
    bool Init(const std::string& dbPath);
    bool SaveMessage(const Message& msg);
    bool GetMessages(const std::string& userId, int limit, std::vector<Message>& messages);
    bool SearchMessages(const std::string& keyword, std::vector<Message>& messages);
};
```

#### 3.3 Bridge通信封装

创建类型安全的Bridge服务层：

```typescript
class BridgeService {
    // 调用C++命令
    static async invoke<T = any>(command: string, params?: any): Promise<T> {
        const json = await window.__tauricpp__.invoke(command, params);
        return JSON.parse(json);
    }
    
    // 监听C++事件
    static listen(event: string, callback: (data: any) => void): () => void {
        return window.__tauricpp__.listen(event, callback);
    }
}
```

### 4. 目录结构设计

```
IPMsgPro/
├── src/                          # [MODIFY] C++后端源码
│   ├── ipmsg/                    # [NEW] IPMsg协议实现
│   │   ├── msgmng.cpp            # 消息管理器（移植自ipmsg-master）
│   │   ├── msgmng.h              # 消息管理器头文件
│   │   ├── protocol.h            # IPMsg协议定义
│   │   ├── network.cpp           # 网络工具函数
│   │   └── network.h             # 网络工具头文件
│   ├── bridge/                   # [NEW] Bridge命令实现
│   │   ├── command_handler.cpp   # 命令处理器
│   │   ├── command_handler.h     # 命令处理器头文件
│   │   ├── user_commands.cpp     # 用户相关命令
│   │   ├── message_commands.cpp  # 消息相关命令
│   │   └── history_commands.cpp  # 历史消息查询命令
│   ├── database/                 # [NEW] 数据库管理
│   │   ├── message_db.cpp        # 消息数据库（SQLite3）
│   │   ├── message_db.h         # 消息数据库头文件
│   │   ├── sqlite3.c            # SQLite3源码（从ipmsg-master复制）
│   │   └── sqlite3.h            # SQLite3头文件
│   ├── file/                     # [NEW] 文件传输
│   │   ├── file_transfer.cpp     # 文件传输管理器
│   │   └── file_transfer.h       # 文件传输头文件
│   └── main.cpp                  # [MODIFY] 应用入口
│
├── frontend/                     # [NEW] React前端
│   ├── src/
│   │   ├── components/           # React组件
│   │   │   ├── LeftSidebar.tsx   # 左侧工具栏
│   │   │   ├── UserListPanel.tsx # 中间用户列表
│   │   │   ├── ChatPanel.tsx     # 右侧对话框
│   │   │   ├── MessageBubble.tsx # 消息气泡
│   │   │   ├── FileTransfer.tsx  # 文件传输组件
│   │   │   └── Settings.tsx      # 设置界面
│   │   ├── stores/               # 状态管理
│   │   │   ├── userStore.ts      # 用户状态
│   │   │   ├── messageStore.ts   # 消息状态
│   │   │   └── configStore.ts    # 配置状态（IndexedDB）
│   │   ├── services/             # 服务层
│   │   │   ├── bridge.ts         # Bridge通信封装
│   │   │   └── configDB.ts       # IndexedDB配置存储服务
│   │   ├── types/                # TypeScript类型定义
│   │   │   └── index.ts          # 类型定义
│   │   ├── App.tsx               # 主应用组件
│   │   └── main.tsx              # 入口文件
│   ├── package.json              # npm配置
│   ├── tsconfig.json             # TypeScript配置
│   ├── vite.config.ts            # Vite配置
│   └── tailwind.config.js        # Tailwind配置
│
├── include/tauricpp/             # [KEEP] TauriCPP头文件
├── CMakeLists.txt                # [MODIFY] 构建配置
└── README.md                     # [NEW] 项目文档
```

### 5. 关键代码结构设计

#### 5.1 C++后端核心接口

```cpp
// src/bridge/command_handler.h
namespace tauricpp {

class CommandHandler {
public:
    static CommandHandler& Instance();
    
    // 注册所有命令
    void RegisterAllCommands();
    
    // 用户管理命令
    json HandleUserDiscover(const json& params);
    json HandleUserList(const json& params);
    
    // 消息传输命令
    json HandleMessageSend(const json& params);
    
    // 文件传输命令
    json HandleFileSend(const json& params);
    
    // 历史消息命令
    json HandleHistoryGet(const json& params);
    json HandleHistorySearch(const json& params);
    json HandleHistoryClear(const json& params);
    
    // 事件推送
    void EmitUserDiscovered(const User& user);
    void EmitMessageReceived(const Message& msg);
    void EmitFileTransferProgress(const TransferProgress& progress);
};

}
```

```cpp
// src/database/message_db.h
#include "sqlite3.h"

namespace tauricpp {

struct MessageRecord {
    std::string id;
    std::string fromId;
    std::string toId;
    std::string content;
    int type;  // 0:text, 1:image, 2:file
    int64_t timestamp;
    int status;
};

class MessageDB {
public:
    MessageDB();
    ~MessageDB();
    
    bool Init(const std::string& dbPath);
    void Close();
    
    // 保存消息
    bool SaveMessage(const MessageRecord& msg);
    
    // 获取与指定用户的历史消息
    bool GetMessages(const std::string& userId, int limit, int offset, 
                    std::vector<MessageRecord>& messages);
    
    // 搜索消息
    bool SearchMessages(const std::string& keyword, 
                       std::vector<MessageRecord>& messages);
    
    // 清空历史消息
    bool ClearMessages(const std::string& userId = "");
    
private:
    sqlite3* db_;
};

}
```

#### 5.2 React前端核心类型定义

```typescript
// frontend/src/types/index.ts

// 用户类型
interface User {
    id: string;
    nickname: string;
    group: string;
    ip: string;
    port: number;
    status: 'online' | 'away' | 'offline';
    version: string;
}

// 消息类型
interface Message {
    id: string;
    from: string;
    to: string;
    content: string;
    type: 'text' | 'image' | 'file';
    timestamp: number;
    status: 'sending' | 'sent' | 'delivered' | 'failed';
}

// 配置类型
interface Config {
    nickname: string;
    password: string;
    segments: string[];  // 多网段配置
    port: number;
    autoDiscovery: boolean;
}

// 文件传输类型
interface FileTransfer {
    id: string;
    filename: string;
    size: number;
    progress: number;
    status: 'pending' | 'transferring' | 'completed' | 'failed';
}

// IndexedDB配置存储服务
class ConfigDB {
    private dbName = 'ipmsg-config';
    private version = 1;
    private storeName = 'config';
    
    async init(): Promise<void>;
    async get<T = any>(key: string): Promise<T | null>;
    async set(key: string, value: any): Promise<void>;
    async remove(key: string): Promise<void>;
    async clear(): Promise<void>;
}
    filename: string;
    size: number;
    progress: number;
    status: 'pending' | 'transferring' | 'completed' | 'failed';
}
```

### 6. 实现注意事项

#### 6.1 性能优化

- **用户发现**: 使用异步UDP广播，避免阻塞UI
- **消息推送**: 使用事件驱动，减少轮询
- **文件传输**: 使用分块传输，支持大文件
- **状态管理**: 使用Zustand的selector，避免不必要的re-render

#### 6.2 错误处理

- **网络错误**: UDP发送失败重试机制
- **文件传输**: 传输中断恢复机制
- **配置错误**: 配置验证和默认值
- **Bridge通信**: 超时和错误处理

#### 6.3 兼容性保证

- **协议兼容**: 严格遵循IPMsg v3.0协议规范
- **编码兼容**: 使用UTF-8编码，兼容中文
- **版本兼容**: 支持不同版本的IPMsg客户端

## 数据存储架构

### 配置存储（IndexedDB）

配置数据存储在前端IndexedDB中，包括：

- 用户名和昵称
- 网段配置（多网段列表）
- 端口号设置
- 自动发现开关
- 其他用户偏好设置

**优势**：

- 前端直接读写，响应速度快
- 无需每次启动都从C++后端加载
- 支持离线访问配置

### 历史消息存储（SQLite3）

所有消息历史存储在C++后端的SQLite3数据库中，包括：

- 发送和接收的文字消息
- 图片消息的缩略图路径
- 文件传输记录
- 消息时间戳和状态

**优势**：

- 支持大量消息存储
- 支持复杂查询（按用户、时间范围、关键词搜索）
- 数据持久化，即使清除IndexedDB也不会丢失

### 数据流图

```
┌─────────────────────────────────────────────────────────┐
│                    React前端                            │
│  ┌──────────────┐              ┌──────────────┐       │
│  │  IndexedDB   │              │  状态管理     │       │
│  │  (配置数据)   │◄────────────►│  (运行时状态) │       │
│  └──────────────┘              └──────────────┘       │
│         ▲                            │                  │
│         │                            │ Bridge调用       │
│         │                            ▼                  │
├─────────┼────────────────────────────────────────────┤
│         │                            │                  │
│         │                            ▼                  │
│  ┌──────┴───────┐         ┌──────────────┐          │
│  │  SQLite3数据库 │◄────────│  C++后端     │          │
│  │ (历史消息)     │         │  (消息处理)   │          │
│  └──────────────┘         └──────────────┘          │
│         ▲                            │                  │
│         │                            │ Winsock2         │
│         │                            ▼                  │
│  ┌──────┴───────┐         ┌──────────────┐          │
│  │  网络层        │         │  IPMsg协议    │          │
│  │ (UDP/TCP)     │◄────────│  (消息传输)   │          │
│  └──────────────┘         └──────────────┘          │
└─────────────────────────────────────────────────────────┘
```

## 实施步骤

### 阶段一：后端核心移植（预计2-3天）

1. 创建`src/ipmsg/`目录，移植MsgMng类
2. 移除Windows GUI依赖，改为回调模式
3. 实现多网段支持
4. 创建Bridge命令处理器
5. 集成SQLite3，实现消息数据库管理
6. 实现历史消息查询Bridge命令
7. 全静态链接

### 阶段二：前端基础架构（预计2-3天）

1. 初始化React + TypeScript + Vite项目
2. 配置Tailwind CSS
3. 实现Bridge通信封装
4. 创建状态管理框架
5. 实现IndexedDB配置存储服务（configDB.ts）
6. 实现配置Store（configStore.ts）

### 阶段三：UI实现（预计3-4天）

1. 实现三栏布局
2. 实现用户列表
3. 实现对话框
4. 实现设置界面

### 阶段四：功能完善（预计3-4天）

1. 实现消息传输
2. 实现文件传输
3. 实现图片预览
4. 实现配置管理（IndexedDB）
5. 实现历史消息存储（SQLite3）
6. 实现历史消息查询和搜索

### 阶段五：测试和优化（预计2-3天）

1. 协议兼容性测试
2. 性能测试和优化
3. UI/UX优化
4. 打包和部署

## 单机测试方案

### 测试方案概述

在单机环境下测试局域网通讯程序，提供以下测试方案：

#### 方案1：本地回环测试模式（推荐用于开发阶段）

**原理**：使用 `127.0.0.1` 作为目标地址，程序同时作为发送方和接收方

**实现方式**：

1. 在配置中添加"本地测试模式"选项
2. 启用时，所有消息发送到 `127.0.0.1`
3. 支持模拟多个虚拟用户

**代码实现**：

```cpp
// src/ipmsg/msgmng.h - 添加本地测试模式支持
class MsgMng {
private:
    bool local_test_mode_;  // 本地测试模式
    std::vector<std::string> test_users_;  // 测试用户列表
    
public:
    void SetLocalTestMode(bool enable) { local_test_mode_ = enable; }
    bool IsLocalTestMode() const { return local_test_mode_; }
    
    // 在本地测试模式下，消息发送到回环地址
    bool SendMessage(const std::string& target, const std::string& msg) {
        if (local_test_mode_) {
            // 发送到 127.0.0.1:port
            return UdpSend("127.0.0.1", port_, msg);
        }
        return UdpSend(target, IPMSG_PORT, msg);
    }
};
```

```typescript
// frontend/src/types/index.ts - 配置类型添加本地测试选项
interface Config {
    nickname: string;
    password: string;
    segments: string[];
    port: number;
    autoDiscovery: boolean;
    localTestMode: boolean;  // 新增：本地测试模式
    localTestUsers: string[];  // 新增：本地测试用户列表
}
```

### 测试配置实现

#### 1. 添加本地测试模式配置

在设置界面添加"本地测试"选项卡：

- 启用/禁用本地测试模式开关
- 添加/删除虚拟用户
- 查看本地测试日志

#### 2. 修改网络发现逻辑

```cpp
// 在本地测试模式下，自动添加虚拟用户
void MsgMng::DiscoverUsers() {
    if (local_test_mode_) {
        // 添加虚拟用户到用户列表
        for (const auto& user : test_users_) {
            User virtualUser;
            virtualUser.id = user;
            virtualUser.nickname = user;
            virtualUser.ip = "127.0.0.1";
            virtualUser.port = port_;
            virtualUser.status = "online";
            EmitUserDiscovered(virtualUser);
        }
        return;
    }
    
    // 正常模式：UDP广播发现用户
    // ...
}
```

#### 3. 添加测试命令

```typescript
// 添加本地测试相关的Bridge命令
| 命令类别 | 命令名称 | 功能描述 |
| --- | --- | --- |
| 测试 | `test.add_user` | 添加测试用户 |
| 测试 | `test.remove_user` | 删除测试用户 |
| 测试 | `test.send_message` | 发送测试消息 |
| 测试 | `test.clear` | 清空测试数据 |
```

### 测试流程图

```
┌─────────────────────────────────────────────────────┐
│                  测试阶段                          │
├─────────────────────────────────────────────────────┤
│  阶段一：本地回环测试                            │
│  - 启用本地测试模式                            │
│  - 添加虚拟用户                                │
│  - 测试消息发送/接收                          │
│  - 测试UI交互                                  │
├─────────────────────────────────────────────────────┤
│  阶段二：虚拟机集成测试                          │
│  - 创建2个虚拟机                               │
│  - 配置虚拟局域网                              │
│  - 测试用户发现                                │
│  - 测试文件传输                                │
├─────────────────────────────────────────────────────┤
│  阶段三：真实网络测试                            │
│  - 在真实局域网环境测试                        │
│  - 与原始IPMsg客户端互操作测试                │
│  - 性能测试                                    │
└─────────────────────────────────────────────────────┘
```

## 风险和控制

### 风险点

1. **协议兼容性**: IPMsg协议细节复杂，可能存在兼容性问题
2. **性能问题**: 大量用户和消息可能导致性能问题
3. **文件传输**: 大文件传输可能不稳定
4. **单机测试局限性**: 无法完全模拟真实网络环境

### 控制措施

1. **协议测试**: 与原始IPMsg客户端进行互操作性测试
2. **性能优化**: 使用异步和事件驱动模式
3. **传输优化**: 实现断点续传和错误恢复
4. **分阶段测试**: 本地测试 → 虚拟机测试 → 真实网络测试

## 总结

本方案采用混合实现策略，将ipmsg-master的核心网络代码移植到TauriCPP框架中，通过Bridge实现与React前端的通信。前端采用类微信的三栏布局，提供现代化的用户体验。方案保证了IPMsg协议的完全兼容性，同时提供了更好的用户界面和扩展性。

## 设计风格

采用类微信的现代简约设计风格，以白色和浅灰色为主色调，配合绿色的点缀色，营造清爽、专业的通讯应用界面。

## 界面布局设计

### 整体布局

采用三栏布局，从左到右依次为：

1. **左侧工具栏**（宽度：60px）：深色背景，包含Logo、导航按钮
2. **中间用户列表**（宽度：300px）：浅色背景，包含搜索、用户列表
3. **右侧对话框**（自适应宽度）：白色背景，包含消息列表和输入区域

### 左侧工具栏设计

- **背景色**: 深色（#2C2C2C）
- **Logo区域**: 顶部，显示应用图标
- **导航按钮**: 垂直排列，包括对话、通讯录、设置按钮
- **用户头像**: 底部，显示当前用户头像
- **交互效果**: 鼠标悬停时按钮高亮，当前选中按钮有高亮指示

### 中间用户列表设计

- **搜索栏**: 顶部，圆角输入框，支持实时搜索
- **刷新按钮**: 搜索栏右侧，圆形按钮
- **用户列表**: 可滚动区域，每个用户显示为卡片
- **用户卡片**: 包含头像、昵称、状态指示点、最后消息预览
- **交互效果**: 鼠标悬停高亮，选中状态有特殊背景色

### 右侧对话框设计

- **顶部信息栏**: 显示当前对话用户的昵称、状态、IP地址
- **消息列表**: 可滚动区域，消息按时间顺序排列
- **消息气泡**: 
- 发送的消息：绿色气泡，右对齐
- 接收的消息：白色气泡，左对齐
- 支持文字、图片、文件等多种消息类型
- **输入区域**: 
- 工具栏：包含表情、图片、文件等按钮
- 文本输入框：支持多行输入，Enter发送，Ctrl+Enter换行
- 发送按钮：绿色，右对齐

### 设置界面设计

- **弹窗形式**: 点击设置按钮后，右侧对话框区域变为设置界面
- **网段设置**: 
- 显示当前网段列表
- 支持添加、删除、编辑网段
- 默认使用当前局域网
- **用户名设置**: 
- 昵称输入框
- 密码输入框（可选）
- **其他设置**: 端口号、自动发现等

## 响应式设计

- **最小宽度**: 800px，小于此宽度显示警告
- **推荐宽度**: 1200px及以上
- **高度**: 自适应，最小高度600px

## 动画效果

- **消息发送**: 发送成功后，消息气泡有轻微的缩放动画
- **用户状态变化**: 状态指示点有呼吸动画
- **文件传输**: 进度条有流动光效
- **按钮交互**: 所有按钮都有悬停和点击动画

## 字体系统

- **字体家族**: "Microsoft YaHei", "PingFang SC", sans-serif
- **标题**: 16px, 500
- **正文**: 14px, 400
- **小字**: 12px, 400

## Agent Extensions

### SubAgent

- **code-explorer**
- Purpose: 探索ipmsg-master和TauriCPP的代码结构，了解现有实现细节
- Expected outcome: 获取完整的代码结构信息，包括关键文件、函数接口、数据结构等，为移植工作提供准确的参考