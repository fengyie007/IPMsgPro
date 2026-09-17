#pragma once
// ============================================================================
// IPMsg Message Manager
// Core network communication class, ported from ipmsg-master/src/msgmng.h
// Removes Windows GUI dependency (HWND), uses callback/event-driven mode
// ============================================================================

#include "protocol.h"
#include "network.h"
#include <string>
#include <vector>
#include <functional>
#include <mutex>
#include <thread>
#include <atomic>
#include <cstdint>
#include <ctime>
#include <optional>

namespace ipmsg {

// ---------- Data Structures ----------

/// User info discovered on the network
struct UserInfo {
    std::string userName;       // login name
    std::string hostName;       // computer name
    std::string nickName;       // display name
    std::string groupName;      // group name
    std::string ipAddress;      // IPv4 address string
    int         portNo = IPMSG_DEFAULT_PORT;
    uint32_t    hostStatus = 0; // IPMSG_ABSENCEOPT etc.
    uint32_t    command = 0;    // last command received
    uint64_t    packetNo = 0;   // last packet number
    time_t      updateTime = 0; // last seen timestamp
    bool        active = false;

    /// Unique key for identifying a user (userName@hostName)
    std::string Key() const {
        return userName + "@" + hostName;
    }
};

/// Parsed message from the network
struct MsgBuf {
    UserInfo    sender;
    uint32_t    command = 0;
    uint64_t    packetNo = 0;
    time_t      timestamp = 0;
    std::string body;       // main message text
    std::string extra;      // extended message (file attach info etc.)
};

// ---------- Callback Types ----------

/// Called when a new user is discovered
using UserDiscoveredCallback = std::function<void(const UserInfo&)>;

/// Called when a user exits or goes offline
using UserLeftCallback = std::function<void(const UserInfo&)>;

/// Called when a message is received
using MessageReceivedCallback = std::function<void(const MsgBuf&)>;

/// Called when a user's status changes
using UserStatusChangedCallback = std::function<void(const UserInfo&)>;

// ---------- MsgMng Class ----------

class MsgMng {
public:
    MsgMng();
    ~MsgMng();

    // Non-copyable
    MsgMng(const MsgMng&) = delete;
    MsgMng& operator=(const MsgMng&) = delete;

    // ---------- Initialization ----------

    /// Initialize with port number and local user info
    /// Returns true on success
    bool Init(int portNo = IPMSG_DEFAULT_PORT,
              const std::string& userName = "",
              const std::string& hostName = "",
              const std::string& nickName = "");

    /// Shutdown and cleanup
    void Shutdown();

    /// Check if initialized successfully
    bool IsReady() const { return ready_; }

    // ---------- Broadcast / Discovery ----------

    /// Broadcast BR_ENTRY to all local network segments
    void BroadcastEntry();
    /// Send BR_ENTRY directly to a specific IP:port (for cross-port discovery)
    void SendDirectEntry(const std::string& ip, int port);

    /// Broadcast BR_EXIT to announce leaving
    void BroadcastExit();

    /// Send BR_ABSENCE to notify status change
    void BroadcastAbsence(uint32_t command);

    /// Add a custom network segment for discovery
    void AddSegment(const std::string& broadcastAddr);

    /// Remove a custom network segment
    void RemoveSegment(const std::string& broadcastAddr);

    /// Get current list of broadcast segments
    std::vector<std::string> GetSegments() const;

    // ---------- Message Sending ----------

    /// Send a text message to a specific user
    bool SendMessage(const UserInfo& target, const std::string& message,
                     uint32_t options = 0);

    /// Send a message with an explicit (raw) command word, bypassing the
    /// automatic IPMSG_SENDMSG | options composition. Required for FeiQ
    /// (飞秋) inline-screenshot fragments, which use command 0x2000C0
    /// (FILEATTACHOPT | 0xC0) and must NOT have IPMSG_SENDMSG OR-ed in.
    bool SendRawCommand(const UserInfo& target, uint32_t command,
                        const std::string& message);

    /// Send a message with file attachment info
    /// @return packetNo of the sent message (0 on failure)
    uint64_t SendMessageWithFile(const UserInfo& target, const std::string& message,
                             const std::string& fileAttachInfo,
                             uint32_t options = IPMSG_FILEATTACHOPT);

    /// Send RECVMSG acknowledgment
    bool SendRecvMsg(const UserInfo& target, uint64_t packetNo);

    // ---------- User Management ----------

    /// Get all currently known users
    std::vector<UserInfo> GetUsers() const;

    /// Find a user by key (userName@hostName)
    std::optional<UserInfo> FindUser(const std::string& key) const;

    /// Update local user info (nickname, group etc.)
    void UpdateLocalInfo(const std::string& nickName, const std::string& groupName);

    /// Get local user info
    const UserInfo& GetLocalUser() const { return localUser_; }
    int GetLocalPort() const { return portNo_; }

    // ---------- Callbacks ----------

    void SetUserDiscoveredCallback(UserDiscoveredCallback cb) { onUserDiscovered_ = std::move(cb); }
    void SetUserLeftCallback(UserLeftCallback cb) { onUserLeft_ = std::move(cb); }
    void SetMessageReceivedCallback(MessageReceivedCallback cb) { onMessageReceived_ = std::move(cb); }
    void SetUserStatusChangedCallback(UserStatusChangedCallback cb) { onUserStatusChanged_ = std::move(cb); }

private:
    // ---------- Internal UDP ----------

    /// Create and bind UDP socket
    bool CreateUdpSocket();

    /// Receive thread main loop
    void ReceiveThreadFunc();

    /// Process a received raw buffer
    void ProcessRecvBuffer(const sockaddr_in& fromAddr, const char* data, int len);

    // ---------- Protocol ----------

    /// Build an IPMsg protocol message
    std::string MakeMsg(uint64_t packetNo, uint32_t command,
                        const std::string& msg, const std::string& extra = "");

    /// Parse a received IPMsg protocol message
    bool ResolveMsg(const char* buf, int size, const std::string& fromIP, int fromPort, MsgBuf& out);

    /// Send UDP data to an address
    bool UdpSend(const std::string& ip, int port, const std::string& data);

    /// Broadcast UDP data to all segments
    void UdpBroadcast(const std::string& data);

    /// Generate a unique packet number
    uint64_t MakePacketNo();

    // ---------- User List Management ----------

    /// Add or update a user in the known users list
    void AddOrUpdateUser(const UserInfo& user);

    /// Remove a user from the known users list
    void RemoveUser(const std::string& key);

private:
    SOCKET udpSock_ = INVALID_SOCKET;
    int portNo_ = IPMSG_DEFAULT_PORT;
    bool ready_ = false;

    UserInfo localUser_;
    std::vector<std::string> segments_;  // custom broadcast segments

    // Known users (key = userName@hostName)
    std::vector<UserInfo> users_;
    mutable std::mutex usersMutex_;

    // Packet number counter - use timestamp as base to avoid collisions after restart
    std::atomic<uint64_t> packetNo_{static_cast<uint64_t>(std::time(nullptr))};

    // Receive thread
    std::thread recvThread_;
    std::atomic<bool> running_{false};

    // Callbacks
    UserDiscoveredCallback onUserDiscovered_;
    UserLeftCallback onUserLeft_;
    MessageReceivedCallback onMessageReceived_;
    UserStatusChangedCallback onUserStatusChanged_;
};

} // namespace ipmsg
