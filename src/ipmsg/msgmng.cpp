// ============================================================================
// IPMsg Message Manager Implementation
// ============================================================================

#include "msgmng.h"
#include "logger.h"
#include <algorithm>
#include <sstream>
#include <chrono>
#include <fstream>
#include <mutex>
#include <chrono>
#include <iomanip>
#include <cctype>

#ifdef _WIN32
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <WinSock2.h>
#include <Windows.h>
#include <ShlObj.h>
#endif

namespace ipmsg {

// ============================================================================
// Encoding Conversion (GBK <-> UTF-8)
// ============================================================================

// Check if a string is valid UTF-8
static bool IsValidUTF8(const std::string& str) {
    const unsigned char* bytes = reinterpret_cast<const unsigned char*>(str.c_str());
    size_t len = str.length();
    size_t i = 0;
    
    while (i < len) {
        unsigned char b = bytes[i];
        int bytes_needed = 0;
        
        if (b < 0x80) {
            bytes_needed = 1;
        } else if ((b & 0xE0) == 0xC0) {
            bytes_needed = 2;
        } else if ((b & 0xF0) == 0xE0) {
            bytes_needed = 3;
        } else if ((b & 0xF8) == 0xF0) {
            bytes_needed = 4;
        } else {
            return false; // Invalid UTF-8 start byte
        }
        
        if (i + bytes_needed > len) return false;
        
        for (int j = 1; j < bytes_needed; ++j) {
            if ((bytes[i + j] & 0xC0) != 0x80) return false;
        }
        
        i += bytes_needed;
    }
    
    return true;
}

// Convert GBK to UTF-8 using Windows API
static std::string GBKToUTF8(const std::string& gbkStr) {
#ifdef _WIN32
    if (gbkStr.empty()) return gbkStr;
    
    // GBK -> UTF-16
    int wlen = MultiByteToWideChar(CP_ACP, 0, gbkStr.c_str(), -1, nullptr, 0);
    if (wlen <= 0) return gbkStr;
    
    std::wstring wstr(wlen - 1, L'\0');
    MultiByteToWideChar(CP_ACP, 0, gbkStr.c_str(), -1, &wstr[0], wlen);
    
    // UTF-16 -> UTF-8
    int ulen = WideCharToMultiByte(CP_UTF8, 0, wstr.c_str(), -1, nullptr, 0, nullptr, nullptr);
    if (ulen <= 0) return gbkStr;
    
    std::string utf8Str(ulen - 1, '\0');
    WideCharToMultiByte(CP_UTF8, 0, wstr.c_str(), -1, &utf8Str[0], ulen, nullptr, nullptr);
    
    return utf8Str;
#else
    return gbkStr; // Fallback: return as-is
#endif
}

// Convert UTF-8 to GBK using Windows API
static std::string UTF8ToGBK(const std::string& utf8Str) {
#ifdef _WIN32
    if (utf8Str.empty()) return utf8Str;
    
    // UTF-8 -> UTF-16
    int wlen = MultiByteToWideChar(CP_UTF8, 0, utf8Str.c_str(), -1, nullptr, 0);
    if (wlen <= 0) return utf8Str;
    
    std::wstring wstr(wlen - 1, L'\0');
    MultiByteToWideChar(CP_UTF8, 0, utf8Str.c_str(), -1, &wstr[0], wlen);
    
    // UTF-16 -> GBK
    int glen = WideCharToMultiByte(CP_ACP, 0, wstr.c_str(), -1, nullptr, 0, nullptr, nullptr);
    if (glen <= 0) return utf8Str;
    
    std::string gbkStr(glen - 1, '\0');
    WideCharToMultiByte(CP_ACP, 0, wstr.c_str(), -1, &gbkStr[0], glen, nullptr, nullptr);
    
    return gbkStr;
#else
    return utf8Str;
#endif
}

// Ensure a string is UTF-8 (convert from GBK if needed)
static std::string EnsureUTF8(const std::string& str) {
    if (str.empty()) return str;
    if (IsValidUTF8(str)) return str;
    // Not valid UTF-8, assume GBK and convert
    return GBKToUTF8(str);
}

MsgMng::MsgMng() = default;

MsgMng::~MsgMng() {
    Shutdown();
}

bool MsgMng::Init(int portNo, const std::string& userName,
                  const std::string& hostName, const std::string& nickName) {
    if (ready_) return true;

    portNo_ = portNo;

    // Setup local user info
    localUser_.userName = userName.empty() ? GetUserName() : userName;
    localUser_.hostName = hostName.empty() ? GetHostName() : hostName;
    localUser_.nickName = nickName.empty() ? localUser_.userName : nickName;
    localUser_.portNo = portNo_;
    localUser_.active = true;
    localUser_.updateTime = std::time(nullptr);

    LogMessage("MSGMNG", "", std::string("[MsgMng] Local user: ") + localUser_.userName + "@" + localUser_.hostName +
               ", nickname=" + localUser_.nickName + ", port=" + std::to_string(portNo_));

    // Auto-detect local IPs
    auto localIPs = GetLocalIPAddresses();
    if (!localIPs.empty()) {
        localUser_.ipAddress = localIPs[0];
        LogMessage("MSGMNG", "", std::string("[MsgMng] Using primary IP: ") + localUser_.ipAddress);
    } else {
        LogMessage("MSGMNG", "", "[MsgMng] WARNING: No local IP addresses found!");
    }

    // Create UDP socket
    if (!CreateUdpSocket()) {
        LogMessage("MSGMNG", "", "[MsgMng] ERROR: Failed to create UDP socket!");
        return false;
    }
    LogMessage("MSGMNG", "", "[MsgMng] UDP socket created and bound to port " + std::to_string(portNo_));

    // Broadcast segments start empty; auto-detected addresses are used
    // directly in UdpBroadcast() via GetAllBroadcastAddresses().
    // Custom segments from user config are added via AddSegment().

    LogMessage("MSGMNG", "", "[MsgMng] Broadcast segments configured:");
    for (const auto& seg : segments_) {
        LogMessage("MSGMNG", "", "[MsgMng]   - " + seg);
    }

    // Start receive thread
    running_ = true;
    recvThread_ = std::thread(&MsgMng::ReceiveThreadFunc, this);
    LogMessage("MSGMNG", "", "[MsgMng] Receive thread started");

    ready_ = true;
    return true;
}

void MsgMng::Shutdown() {
    // Stop any in-flight IP range scan and reap its thread first: destroying a
    // joinable std::thread calls std::terminate. This runs before the ready_
    // check because a scan can be started even when Init() failed.
    scanning_ = false;
    if (scanThread_.joinable()) {
        scanThread_.join();
    }

    if (!ready_) return;

    // Broadcast exit
    if (udpSock_ != INVALID_SOCKET) {
        BroadcastExit();
    }

    running_ = false;

    // Close socket to unblock recv
    if (udpSock_ != INVALID_SOCKET) {
        closesocket(udpSock_);
        udpSock_ = INVALID_SOCKET;
    }

    if (recvThread_.joinable()) {
        recvThread_.join();
    }

    ready_ = false;
}

bool MsgMng::CreateUdpSocket() {
    udpSock_ = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (udpSock_ == INVALID_SOCKET) {
        return false;
    }

    // Allow broadcast
    BOOL bBroadcast = TRUE;
    setsockopt(udpSock_, SOL_SOCKET, SO_BROADCAST,
               reinterpret_cast<const char*>(&bBroadcast), sizeof(bBroadcast));

    // Allow address reuse
    BOOL bReuse = TRUE;
    setsockopt(udpSock_, SOL_SOCKET, SO_REUSEADDR,
               reinterpret_cast<const char*>(&bReuse), sizeof(bReuse));

    // Bind
    sockaddr_in addr = {};
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_ANY);
    addr.sin_port = htons(static_cast<u_short>(portNo_));

    if (bind(udpSock_, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) == SOCKET_ERROR) {
        closesocket(udpSock_);
        udpSock_ = INVALID_SOCKET;
        return false;
    }

    // Set recv buffer size
    int bufSize = MAX_SOCKBUF;
    setsockopt(udpSock_, SOL_SOCKET, SO_RCVBUF,
               reinterpret_cast<const char*>(&bufSize), sizeof(bufSize));

    return true;
}

void MsgMng::ReceiveThreadFunc() {
    char buf[MAX_UDPBUF];
    sockaddr_in fromAddr = {};
    int fromLen = sizeof(fromAddr);

    while (running_) {
        int recvLen = recvfrom(udpSock_, buf, sizeof(buf) - 1, 0,
                               reinterpret_cast<sockaddr*>(&fromAddr), &fromLen);
        if (recvLen <= 0) {
            if (!running_) break;
            continue;
        }

        buf[recvLen] = '\0';

        char fromIP[INET_ADDRSTRLEN] = {};
        inet_ntop(AF_INET, &fromAddr.sin_addr, fromIP, sizeof(fromIP));
        int fromPort = ntohs(fromAddr.sin_port);

        ProcessRecvBuffer(fromAddr, buf, recvLen);
    }
}

void MsgMng::ProcessRecvBuffer(const sockaddr_in& fromAddr, const char* data, int len) {
    char fromIP[INET_ADDRSTRLEN] = {};
    inet_ntop(AF_INET, &fromAddr.sin_addr, fromIP, sizeof(fromIP));
    int fromPort = ntohs(fromAddr.sin_port);

    MsgBuf msg;
    if (!ResolveMsg(data, len, fromIP, fromPort, msg)) {
        return;
    }

    // Skip own messages.
    // A message is "own" only if it has the same key AND the same port.
    // Same user@host on a different port means it's a different instance (e.g. two IPMsgPro instances).
    const UserInfo local = LocalUserSnapshot();
    if (msg.sender.Key() == local.Key() && msg.sender.portNo == local.portNo) {
        return;
    }

    // Fix: replace UDP ephemeral port with known listening port if user is already in our list.
    // The msg.sender.portNo is set from the UDP recvfrom source port (ephemeral).
    // For RECVMSG replies, ANSENTRY, SENDMSG and file transfers, we need the sender's actual listening port.
    {
        auto knownUser = FindUser(msg.sender.Key());
        if (knownUser && knownUser->portNo != msg.sender.portNo) {
            msg.sender.portNo = knownUser->portNo;
        } else if (!knownUser && msg.sender.ipAddress == "127.0.0.1") {
            // When running two instances on the same machine, the hostName
            // ("localhost") may differ from the actual hostName broadcast by the other instance.
            // Try matching by IP address to find the correct listening port.
            auto users = GetUsers();  // GetUsers is always const
            for (const auto& u : users) {
                if (u.ipAddress == "127.0.0.1" && u.portNo != msg.sender.portNo &&
                    u.portNo != portNo_ && u.portNo > 0) {
                    msg.sender.portNo = u.portNo;
                    break;
                }
            }
        }
    }

    uint32_t mode = GET_MODE(msg.command);

    switch (mode) {
    case IPMSG_BR_ENTRY: {
        // New user discovered
        msg.sender.active = true;
        msg.sender.updateTime = std::time(nullptr);
        AddOrUpdateUser(msg.sender);

        LogMessage("MSGMNG", "", std::string("[MsgMng] USER DISCOVERED (BR_ENTRY): ")
                  + msg.sender.userName + "@" + msg.sender.hostName
                  + " (" + msg.sender.ipAddress + ":" + std::to_string(msg.sender.portNo) + ")"
                  + ", nickname=" + msg.sender.nickName
                  + ", group=" + msg.sender.groupName);

        if (onUserDiscovered_) {
            onUserDiscovered_(msg.sender);
        }

        // Reply with ANSENTRY
        {
            std::string body = local.nickName;
            std::string extra = local.groupName;
            auto reply = MakeMsg(MakePacketNo(),
                IPMSG_ANSENTRY | IPMSG_CAPUTF8OPT,
                body, extra);
            // Send to sender's UDP source port (original behavior)
            UdpSend(msg.sender.ipAddress, msg.sender.portNo, reply);
            // If the sender appears to be on the same host, also try their default port.
            // This enables cross-instance discovery on the same machine with different ports.
            if (msg.sender.ipAddress == "127.0.0.1" || msg.sender.ipAddress == "0.0.0.0") {
                // Try common ports: 2425 (default), 2525 (test server), and portNo_ (our port)
                for (int tryPort : {2425, 2525, portNo_}) {
                    if (tryPort != msg.sender.portNo && tryPort != portNo_) {
                        UdpSend(msg.sender.ipAddress, tryPort, reply);
                    }
                }
            }
        }
        break;
    }

    case IPMSG_ANSENTRY: {
        // Response to our BR_ENTRY
        msg.sender.active = true;
        msg.sender.updateTime = std::time(nullptr);
        AddOrUpdateUser(msg.sender);

        LogMessage("MSGMNG", "", std::string("[MsgMng] USER DISCOVERED (ANSENTRY): ")
                  + msg.sender.userName + "@" + msg.sender.hostName
                  + " (" + msg.sender.ipAddress + ":" + std::to_string(msg.sender.portNo) + ")"
                  + ", nickname=" + msg.sender.nickName
                  + ", group=" + msg.sender.groupName);

        if (onUserDiscovered_) {
            onUserDiscovered_(msg.sender);
        }
        break;
    }

    case IPMSG_BR_EXIT: {
        RemoveUser(msg.sender.Key());
        if (onUserLeft_) {
            onUserLeft_(msg.sender);
        }
        break;
    }

    case IPMSG_BR_ABSENCE: {
        msg.sender.hostStatus = GET_OPT(msg.command);
        msg.sender.updateTime = std::time(nullptr);
        AddOrUpdateUser(msg.sender);

        if (onUserStatusChanged_) {
            onUserStatusChanged_(msg.sender);
        }
        break;
    }

    case IPMSG_SENDMSG: {
        // Incoming message
        if (onMessageReceived_) {
            onMessageReceived_(msg);
        }

        // Auto-acknowledge if requested
        if (msg.command & IPMSG_SENDCHECKOPT) {
            // Fix: use known user port (listening port) instead of ephemeral UDP source port.
            // The msg.sender.portNo was set from the UDP recvfrom source port, which is a
            // temporary port. Reply RECVMSG to the sender's actual listening port so
            // the other instance can receive it on its UDP listening socket.
            UserInfo replyTarget = msg.sender;
            auto knownUser = FindUser(msg.sender.Key());
            if (knownUser && knownUser->portNo != replyTarget.portNo) {
                replyTarget.portNo = knownUser->portNo;
            }
            SendRecvMsg(replyTarget, msg.packetNo);
        }
        break;
    }

    case IPMSG_RECVMSG: {
        // Acknowledgment received - forward to callback
        if (onMessageReceived_) {
            onMessageReceived_(msg);
        }
        break;
    }

    default:
        // FeiQ (飞秋) inline screenshot fragments are sent with a non-standard
        // command (0x2000C0 = FILEATTACHOPT + vendor mode bits) that never
        // matches a standard IPMsg mode. Forward them to the upper layer so the
        // bridge can reassemble the image. FeiQ fragment body format is:
        //   "<hexid>|<totalSize>|<offset>|<fragCount>|<fragIndex>|<fragSize>|0|2|0|<mtime>#<data>"
        if ((msg.command & IPMSG_FILEATTACHOPT) != 0 && !msg.body.empty() &&
            msg.body.find('|') != std::string::npos &&
            msg.body.find('#') != std::string::npos &&
            std::isxdigit((unsigned char)msg.body[0])) {
            if (onMessageReceived_) {
                onMessageReceived_(msg);
            }
        }
        break;
    }
}

// ---------- Protocol ----------

// Build a FeiQ (飞秋) style extended version field so our packets look like
// a FeiQ peer to other clients. Format (same as FeiQ BR_ENTRY):
//   1_lbt6_0#128#<MAC>#0#0#0#4001#9
//   - 1            : IPMsg protocol version (fixed)
//   - _lbt6_       : FeiQ capability tag (enhanced broadcast)
//   - 0            : flag
//   - 128          : user-limit / flag
//   - <MAC>        : local adapter MAC, uppercase, no separators
//   - 0/0/0        : reserved
//   - 4001         : FeiQ software version (4.0.0.1)
//   - 9            : build number
// The leading "1" is what the standard IPMsg parser reads as the version,
// so existing peers still interoperate.
static std::string BuildFeiQVersion() {
    static std::string cached;
    if (!cached.empty()) return cached;

    std::string mac = ipmsg::GetLocalMacAddress();
    if (mac.empty()) mac = "000000000000";
    cached = "1_lbt6_0#128#" + mac + "#0#0#0#4001#9";
    return cached;
}

std::string MsgMng::MakeMsg(uint64_t packetNo, uint32_t command,
                             const std::string& msg, const std::string& extra) {
    // IPMsg protocol format: "1:packetNo:userName:hostName:command:body\0extra\0extInfo"
    // Compatible with original IPMsg v3 protocol

    uint32_t mode = GET_MODE(command);
    bool isBrCmd = (mode == IPMSG_BR_ENTRY || mode == IPMSG_BR_EXIT ||
                    mode == IPMSG_BR_ABSENCE);
    bool isAnsEntry = (mode == IPMSG_ANSENTRY);
    bool isUtf8 = (command & IPMSG_UTF8OPT) != 0;

    // Local user fields are stored as UTF-8 (the frontend sends UTF-8). When
    // broadcasting without IPMSG_UTF8OPT we must encode the wire payload as GBK
    // so clients such as FeiQ display Chinese names correctly instead of mojibake.
    // The isUtf8 branch keeps the legacy GBK->UTF-8 conversion for completeness.
    auto toWire = [&](const std::string& s) -> std::string {
        return isUtf8 ? GBKToUTF8(s) : UTF8ToGBK(s);
    };

    // MakeMsg runs on the UI, receive and scan threads; read a consistent copy.
    const UserInfo local = LocalUserSnapshot();
    std::string userName = toWire(local.userName);
    std::string hostName = toWire(local.hostName);
    std::string nickName = toWire(local.nickName);
    std::string groupName = toWire(local.groupName);

    // Build header: "ver:packetNo:userName:hostName:command:"
    std::string result;
    result.reserve(MAX_UDPBUF);

    // Use FeiQ-style extended version field for better compatibility with FeiQ peers.
    result += BuildFeiQVersion() + ":";
    result += std::to_string(packetNo) + ":";
    result += userName + ":";
    result += hostName + ":";
    result += std::to_string(command) + ":";

    // Append body (message text or nickname for BR commands)
    result += toWire(msg);

    // Append extra (separated by '\0'). Only encode for local-user info packets
    // (group name); file-attach info must stay raw.
    bool isLocalInfo = isBrCmd || isAnsEntry;
    std::string extraOut = isLocalInfo ? toWire(extra) : extra;
    if (!extraOut.empty()) {
        result += std::string(1, '\0');
        result += extraOut;
    }

    // For BR_ENTRY/BR_EXIT/BR_NOTIFY, append extended info after another '\0'
    // Format: \nUN:username\nHN:hostname\nNN:nickname\nGN:group\nVS:version
    if (isBrCmd) {
        result += std::string(1, '\0');
        result += "\nUN:" + userName;
        result += "\nHN:" + hostName;
        result += "\nNN:" + nickName;
        result += "\nGN:" + groupName;
        result += "\nVS:0x0003";
    }
    else if (isAnsEntry) {
        result += std::string(1, '\0');
        result += "\nVS:0x0003";
    }

    return result;
}

bool MsgMng::ResolveMsg(const char* buf, int size,
                         const std::string& fromIP, int fromPort, MsgBuf& out) {
    // IPMsg protocol format: "ver:packetNo:userName:hostName:command:body[\0extra[\0extInfo]]"
    if (size <= 0) return false;

    // Log raw message for debugging
    std::string rawMsg(buf, size);
    LogMessage("MSGMNG", "", "========== RAW MESSAGE START (" + std::to_string(size) + " bytes from " + fromIP + ":" + std::to_string(fromPort) + ") ==========");
    
    // Print raw hex dump
    std::stringstream hexDump;
    for (int i = 0; i < size; ++i) {
        hexDump << std::hex << std::setfill('0') << std::setw(2) 
                << (unsigned int)(unsigned char)buf[i] << " ";
        if ((i + 1) % 16 == 0 || i == size - 1) {
            hexDump << " | ";
            int start = (i / 16) * 16;
            for (int j = start; j <= i; ++j) {
                unsigned char c = buf[j];
                hexDump << (c >= 32 && c < 127 ? (char)c : '.');
            }
            LogMessage("MSGMNG", "", hexDump.str());
            hexDump.str("");
        }
    }
    
    // Print protocol fields
    LogMessage("MSGMNG", "", "Raw string (first 200 chars): " + rawMsg.substr(0, 200));
    LogMessage("MSGMNG", "", "========== RAW MESSAGE END ==========");
    
    // Parse header by finding the first 5 colon-separated fields
    // Fields: version, packetNo, userName, hostName, command
    // Everything after the 5th colon is the body
    const char* p = buf;
    const char* end = buf + size;
    std::string fields[5];

    for (int i = 0; i < 5; ++i) {
        const char* colon = static_cast<const char*>(memchr(p, ':', end - p));
        if (!colon) return false;

        fields[i] = std::string(p, colon - p);
        p = colon + 1;
    }

    // p now points to the body
    const char* bodyPtr = p;
    int bodyLen = static_cast<int>(end - bodyPtr);

    try {
        // Handle FeiQ extended version format: "1_lbt6_0#128#30B49EAE34C4#0#0#0#4001#9"
        // Extract only the numeric part before the first non-digit character
        std::string verStr = fields[0];
        size_t verEnd = verStr.find_first_not_of("0123456789");
        if (verEnd != std::string::npos) {
            verStr = verStr.substr(0, verEnd);
            LogMessage("MSGMNG", "", "FeiQ extended version detected: " + fields[0] + ", using: " + verStr);
        }
        int version = std::stoi(verStr);
        if (version != IPMSG_VERSION) return false;

        out.packetNo = std::stoull(fields[1]);
        
        // Convert username and hostname to UTF-8 if needed
        out.sender.userName = EnsureUTF8(fields[2]);
        out.sender.hostName = EnsureUTF8(fields[3]);
        
        out.command = std::stoul(fields[4]);
        out.sender.ipAddress = fromIP;
        out.sender.portNo = fromPort;
        out.timestamp = std::time(nullptr);
        
        LogMessage("MSGMNG", "", "Parsed header: ver=" + fields[0] + " pkt=" + fields[1] + 
               " user=" + out.sender.userName + " host=" + out.sender.hostName + 
               " cmd=" + std::to_string(out.command));
    } catch (...) {
        LogMessage("MSGMNG", "", "Failed to parse header");
        return false;
    }

    uint32_t mode = GET_MODE(out.command);

    // Find null byte separating body from extra.
    // NOTE: FeiQ inline screenshot fragments (cmd FILEATTACHOPT + non-standard mode)
    // carry the raw JPEG bytes after a '#' separator, and those bytes may contain
    // embedded NULs (JPEG frequently does). strnlen would truncate at the first NUL
    // and we'd lose the entire image. So for FeiQ fragments we keep the full raw
    // payload (bodyPtr .. size) without truncation.
    bool isFeiQFragment = (out.command & IPMSG_FILEATTACHOPT) != 0 &&
                          bodyLen > 0 &&
                          std::isxdigit((unsigned char)bodyPtr[0]) &&
                          std::memchr(bodyPtr, '#', bodyLen) != nullptr;

    int bodyStrLen;
    if (isFeiQFragment) {
        bodyStrLen = bodyLen;  // keep embedded NULs intact
    } else {
        bodyStrLen = static_cast<int>(strnlen(bodyPtr, bodyLen));
    }
    out.body = std::string(bodyPtr, bodyStrLen);

    // Convert body to UTF-8 if sender is not using UTF-8 (e.g. FeiQ sends GBK)
    // Only convert SENDMSG bodies (not BR_* messages)
    if (mode == IPMSG_SENDMSG || mode == IPMSG_RECVMSG) {
        if (!(out.command & IPMSG_UTF8OPT)) {
            out.body = EnsureUTF8(out.body);
        }
    }

    // Find extra (after first null byte)
    const char* extraPtr = nullptr;
    int extraLen = 0;
    if (bodyStrLen + 1 < bodyLen) {
        extraPtr = bodyPtr + bodyStrLen + 1;
        int remaining = bodyLen - bodyStrLen - 1;
        extraLen = static_cast<int>(strnlen(extraPtr, remaining));
        out.extra = std::string(extraPtr, extraLen);
    }

    // Find extInfo (after second null byte) — for CAPUTF8OPT messages
    const char* extInfoPtr = nullptr;
    int extInfoLen = 0;
    if (extraPtr && (extraPtr + extraLen + 1) < end) {
        extInfoPtr = extraPtr + extraLen + 1;
        extInfoLen = static_cast<int>(end - extInfoPtr);
    }

    // Parse nickname and group for BR_ENTRY / BR_NOTIFY / ANSENTRY
    if (mode == IPMSG_BR_ENTRY || mode == IPMSG_BR_ABSENCE ||
        mode == IPMSG_ANSENTRY) {
        // Nickname is in the body - convert to UTF-8 if needed
        out.sender.nickName = EnsureUTF8(out.body);

        // Group is in the extra (simple format)
        if (!out.extra.empty() && out.extra[0] != '\n') {
            out.sender.groupName = EnsureUTF8(out.extra);
        }
    }

    // Parse extended info lines (\nUN:...\nHN:...\nNN:...\nGN:...\nVS:...)
    if (extInfoPtr && extInfoLen > 0 && (out.command & IPMSG_CAPUTF8OPT)) {
        std::string extInfo(extInfoPtr, extInfoLen);
        std::istringstream iss(extInfo);
        std::string line;
        while (std::getline(iss, line)) {
            if (line.empty()) continue;

            if (line.compare(0, 3, "UN:") == 0) {
                out.sender.userName = EnsureUTF8(line.substr(3));
            } else if (line.compare(0, 3, "HN:") == 0) {
                out.sender.hostName = EnsureUTF8(line.substr(3));
            } else if (line.compare(0, 3, "NN:") == 0) {
                out.sender.nickName = EnsureUTF8(line.substr(3));
            } else if (line.compare(0, 3, "GN:") == 0) {
                out.sender.groupName = EnsureUTF8(line.substr(3));
            }
            // VS: version info - ignored for now
        }
    }

    out.sender.hostStatus = GET_OPT(out.command);
    
    LogMessage("MSGMNG", "", "Resolved message: mode=" + std::to_string(mode) + 
           " nick=" + out.sender.nickName + " group=" + out.sender.groupName);

    return true;
}

bool MsgMng::UdpSend(const std::string& ip, int port, const std::string& data) {
    if (udpSock_ == INVALID_SOCKET) return false;

    sockaddr_in addr = {};
    addr.sin_family = AF_INET;
    inet_pton(AF_INET, ip.c_str(), &addr.sin_addr);
    addr.sin_port = htons(static_cast<u_short>(port));

    int sent = sendto(udpSock_, data.data(), static_cast<int>(data.size()), 0,
                      reinterpret_cast<sockaddr*>(&addr), sizeof(addr));

    return sent != SOCKET_ERROR;
}

void MsgMng::UdpBroadcast(const std::string& data) {
    // Broadcast to all auto-detected segments
    auto broadcasts = GetAllBroadcastAddresses();

    for (const auto& bc : broadcasts) {
        bool sent = UdpSend(bc, portNo_, data);
        LogMessage("MSGMNG", "", "[MsgMng] Broadcast to " + bc + ":" + std::to_string(portNo_) +
                   " (size=" + std::to_string(data.size()) + " bytes) " + (sent ? "OK" : "FAILED"));
    }

    // Also broadcast to custom segments
    for (const auto& seg : segments_) {
        bool sent = UdpSend(seg, portNo_, data);
        LogMessage("MSGMNG", "", "[MsgMng] Broadcast to " + seg + ":" + std::to_string(portNo_) +
                   " (size=" + std::to_string(data.size()) + " bytes) " + (sent ? "OK" : "FAILED"));
    }
}

uint64_t MsgMng::MakePacketNo() {
    return ++packetNo_;
}

// ---------- Broadcast / Discovery ----------

void MsgMng::BroadcastEntry() {
    // Body: nickname, Extra: groupname
    const UserInfo local = LocalUserSnapshot();
    std::string body = local.nickName;
    std::string extra = local.groupName;
    auto msg = MakeMsg(MakePacketNo(),
        IPMSG_BR_ENTRY | IPMSG_CAPUTF8OPT,
        body, extra);
    
    LogMessage("MSGMNG", "", "[MsgMng] Broadcasting BR_ENTRY to " + std::to_string(segments_.size()) + " segments...");
    UdpBroadcast(msg);
}

void MsgMng::SendDirectEntry(const std::string& ip, int port) {
    const UserInfo local = LocalUserSnapshot();
    std::string body = local.nickName;
    std::string extra = local.groupName;
    auto msg = MakeMsg(MakePacketNo(),
        IPMSG_BR_ENTRY | IPMSG_CAPUTF8OPT,
        body, extra);
    UdpSend(ip, port, msg);
}

void MsgMng::BroadcastExit() {
    auto msg = MakeMsg(MakePacketNo(),
        IPMSG_BR_EXIT,
        LocalUserSnapshot().nickName);
    UdpBroadcast(msg);
}

void MsgMng::BroadcastAbsence(uint32_t command) {
    const UserInfo local = LocalUserSnapshot();
    std::string body = local.nickName;
    std::string extra = local.groupName;
    auto msg = MakeMsg(MakePacketNo(), command, body, extra);
    UdpBroadcast(msg);
}

void MsgMng::AddSegment(const std::string& broadcastAddr) {
    auto it = std::find(segments_.begin(), segments_.end(), broadcastAddr);
    if (it == segments_.end()) {
        segments_.push_back(broadcastAddr);
    }
}

void MsgMng::RemoveSegment(const std::string& broadcastAddr) {
    segments_.erase(
        std::remove(segments_.begin(), segments_.end(), broadcastAddr),
        segments_.end());
}

std::vector<std::string> MsgMng::GetSegments() const {
    return segments_;
}

void MsgMng::AddDirectUser(const std::string& ip, int port) {
    // Check if already exists
    auto it = std::find_if(directUsers_.begin(), directUsers_.end(),
                           [&](const auto& p) { return p.first == ip && p.second == port; });
    if (it == directUsers_.end()) {
        directUsers_.emplace_back(ip, port);
        LogMessage("MSGMNG", "", "[MsgMng] Added direct user: " + ip + ":" + std::to_string(port));
    }
}

std::vector<std::pair<std::string, int>> MsgMng::GetDirectUsers() const {
    return directUsers_;
}

void MsgMng::ClearDirectUsers() {
    directUsers_.clear();
    LogMessage("MSGMNG", "", "[MsgMng] Cleared all direct users");
}

void MsgMng::ClearScanRanges() {
    scanRanges_.clear();
    LogMessage("MSGMNG", "", "[MsgMng] Cleared all scan ranges");
}

void MsgMng::AddScanRange(const std::string& range) {
    if (std::find(scanRanges_.begin(), scanRanges_.end(), range) == scanRanges_.end()) {
        scanRanges_.push_back(range);
        LogMessage("MSGMNG", "", "[MsgMng] Added scan range: " + range);
    }
}

std::vector<std::string> MsgMng::GetScanRanges() const {
    return scanRanges_;
}

// ---------- Message Sending ----------

uint64_t MsgMng::SendMessage(const UserInfo& target, const std::string& message,
                             uint32_t options) {
    // Wire encoding (UTF-8 from frontend -> GBK for FeiQ compatibility) is
    // handled centrally in MakeMsg, so pass the message through as-is.
    uint32_t cmd = IPMSG_SENDMSG | options;
    uint64_t pktNo = MakePacketNo();
    auto msg = MakeMsg(pktNo, cmd, message);
    bool ok = UdpSend(target.ipAddress, target.portNo, msg);

    LogMessage("MSGMNG", "", "[MsgMng] SendMessage to " + target.Key() +
               " (" + target.ipAddress + ":" + std::to_string(target.portNo) + ")"
               + " len=" + std::to_string(message.size()) + " cmd=0x" +
               ([](uint32_t v)->std::string{std::ostringstream o;o<<std::hex<<v;return o.str();})(cmd) +
               " pkt=" + std::to_string(pktNo) +
               " ok=" + (ok ? "true" : "false"));

    return ok ? pktNo : 0;
}

bool MsgMng::SendRawCommand(const UserInfo& target, uint32_t command,
                            const std::string& message) {
    // Send with the exact command word given (no IPMSG_SENDMSG OR-ed in),
    // so FeiQ inline-screenshot fragments keep their 0x2000C0 mode byte.
    auto msg = MakeMsg(MakePacketNo(), command, message);
    bool ok = UdpSend(target.ipAddress, target.portNo, msg);

    LogMessage("MSGMNG", "", "[MsgMng] SendRawCommand to " + target.Key() +
               " (" + target.ipAddress + ":" + std::to_string(target.portNo) + ")"
               + " len=" + std::to_string(message.size()) + " cmd=0x" +
               ([](uint32_t v)->std::string{std::ostringstream o;o<<std::hex<<v;return o.str();})(command) +
               " ok=" + (ok ? "true" : "false"));

    return ok;
}

uint64_t MsgMng::SendMessageWithFile(const UserInfo& target, const std::string& message,
                                      const std::string& fileAttachInfo, uint32_t options) {
    // Don't use IPMSG_UTF8OPT for file messages - FeiQ doesn't support it
    // FeiQ uses GBK encoding for file messages
    uint32_t cmd = IPMSG_SENDMSG | IPMSG_FILEATTACHOPT | options;
    uint64_t pktNo = MakePacketNo();
    auto msg = MakeMsg(pktNo, cmd, message, fileAttachInfo);
    
    // Debug: print the raw message for analysis (to both console and debug log)
    {
        std::string dbgMsg;
        for (size_t i = 0; i < msg.size() && i < 300; i++) {
            if (msg[i] == '\0') dbgMsg += "\\0";
            else if (msg[i] == '\x07') dbgMsg += "\\a";
            else dbgMsg += msg[i];
        }
        LogMessage("MSGMNG", "", "[SEND-FILE] pktNo=" + std::to_string(pktNo) + ", cmd=0x" +
                   ([](uint32_t v)->std::string{std::ostringstream o;o<<std::hex<<v;return o.str();})(cmd) +
                   ", msgLen=" + std::to_string(msg.size()));
        LogMessage("MSGMNG", "", "[SEND-FILE] rawMsg=" + dbgMsg);
        // Also write to debug log for file-based analysis
        LogMessage("MSGMNG", "", "[SEND-FILE-RAW] pktNo=" + std::to_string(pktNo) + ", cmd=0x" + 
               ([](uint32_t v)->std::string{std::ostringstream o;o<<std::hex<<v;return o.str();})(cmd) +
               ", rawMsg=" + dbgMsg);
    }
    
    bool ok = UdpSend(target.ipAddress, target.portNo, msg);
    return ok ? pktNo : 0;
}

bool MsgMng::SendRecvMsg(const UserInfo& target, uint64_t pktNo) {
    auto msg = MakeMsg(MakePacketNo(), IPMSG_RECVMSG, std::to_string(pktNo));
    return UdpSend(target.ipAddress, target.portNo, msg);
}

// ---------- User Management ----------

std::vector<UserInfo> MsgMng::GetUsers() const {
    std::lock_guard<std::mutex> lock(usersMutex_);
    return users_;
}

std::optional<UserInfo> MsgMng::FindUser(const std::string& key) const {
    std::lock_guard<std::mutex> lock(usersMutex_);
    for (const auto& u : users_) {
        if (u.Key() == key) return u;
    }
    return std::nullopt;
}

void MsgMng::UpdateLocalInfo(const std::optional<std::string>& nickName,
                             const std::optional<std::string>& groupName) {
    bool changed = false;
    {
        std::lock_guard<std::mutex> lock(localUserMutex_);
        if (nickName) {
            // Same fallback as Init(): peers must never see an empty nickname.
            std::string nn = nickName->empty() ? localUser_.userName : *nickName;
            if (nn != localUser_.nickName) {
                localUser_.nickName = nn;
                changed = true;
            }
        }
        if (groupName && *groupName != localUser_.groupName) {
            localUser_.groupName = *groupName;
            changed = true;
        }
    }

    // Re-broadcast entry so peers pick up the new info (outside the lock:
    // MakeMsg takes its own snapshot).
    if (changed && ready_) {
        BroadcastEntry();
    }
}

UserInfo MsgMng::LocalUserSnapshot() const {
    std::lock_guard<std::mutex> lock(localUserMutex_);
    return localUser_;
}

void MsgMng::AddOrUpdateUser(const UserInfo& user) {
    std::lock_guard<std::mutex> lock(usersMutex_);
    for (auto& u : users_) {
        if (u.Key() == user.Key()) {
            u = user;
            return;
        }
    }
    users_.push_back(user);

    // If we're actively scanning, increment found counter for new users
    if (scanning_) {
        scanFoundCount_++;
        LogMessage("MSGMNG", "", "[Scanner] Found new user during scan: " + user.nickName + "@" + user.hostName + " (" + user.ipAddress + ")");
    }
}

void MsgMng::RemoveUser(const std::string& key) {
    std::lock_guard<std::mutex> lock(usersMutex_);
    users_.erase(
        std::remove_if(users_.begin(), users_.end(),
            [&key](const UserInfo& u) { return u.Key() == key; }),
        users_.end());
}

// ============================================================================
// IP Range Scanner (active scanning for cross-subnet users)
// ============================================================================

bool MsgMng::ScanIpRange(const std::string& startIp, const std::string& endIp,
                         int port, int delayMs) {
    // Backward compatibility: single range scan
    return ScanIpRanges({startIp + "-" + endIp}, port, delayMs);
}

bool MsgMng::ScanIpRanges(const std::vector<std::string>& ranges, int port, int delayMs) {
    // Check if already scanning
    bool expected = false;
    if (!scanning_.compare_exchange_strong(expected, true)) {
        LogMessage("MSGMNG", "", "[Scanner] Scan already in progress, ignoring");
        return false;
    }

    // Reap the previous scan thread. A scan that finished on its own leaves
    // scanThread_ joinable; assigning a new std::thread over a joinable one
    // calls std::terminate.
    if (scanThread_.joinable()) {
        scanThread_.join();
    }

    // Clamp delay
    if (delayMs < 10) delayMs = 10;
    if (delayMs > 5000) delayMs = 5000;

    // Use provided ranges or fall back to configured scanRanges_
    const auto& rangesToScan = ranges.empty() ? scanRanges_ : ranges;
    if (rangesToScan.empty()) {
        scanning_ = false;
        LogMessage("MSGMNG", "", "[Scanner] No IP ranges configured for scan");
        return false;
    }

    LogMessage("MSGMNG", "", "[Scanner] Starting scan of " + std::to_string(rangesToScan.size()) + " ranges, port=" + std::to_string(port) + ", delay=" + std::to_string(delayMs) + "ms");

    // Start scanning thread
    scanThread_ = std::thread([this, rangesToScan, port, delayMs]() {
        uint32_t totalFound = 0;

        for (const auto& range : rangesToScan) {
            if (!scanning_) break;

            // Parse range: "startIp-endIp" (use rfind to find LAST dash, since IP contains dots)
            // Supports both full IP format "10.8.33.1-10.8.33.254" and shorthand "10.8.33.1-254"
            size_t dashPos = range.rfind('-');
            if (dashPos == std::string::npos) {
                LogMessage("MSGMNG", "", "[Scanner] Invalid range format: " + range);
                continue;
            }

            std::string startIp = range.substr(0, dashPos);
            std::string endIp = range.substr(dashPos + 1);

            uint32_t start = ipmsg::IPToUint32(startIp);
            uint32_t end;

            // Check if endIp is just a last octet (0-255) - shorthand format
            bool isShorthand = false;
            try {
                int lastOctet = std::stoi(endIp);
                if (lastOctet >= 0 && lastOctet <= 255) {
                    isShorthand = true;
                }
            } catch (...) {
                isShorthand = false;
            }

            if (isShorthand) {
                // Construct full end IP from startIp's first three octets + lastOctet
                size_t lastDot = startIp.rfind('.');
                if (lastDot != std::string::npos) {
                    std::string prefix = startIp.substr(0, lastDot + 1);
                    end = ipmsg::IPToUint32(prefix + endIp);
                } else {
                    LogMessage("MSGMNG", "", "[Scanner] Invalid start IP for shorthand range: " + range);
                    continue;
                }
            } else {
                end = ipmsg::IPToUint32(endIp);
            }

            if (start == 0 || end == 0 || end < start) {
                LogMessage("MSGMNG", "", "[Scanner] Invalid IP range: " + range);
                continue;
            }

            uint32_t rangeTotal = end - start + 1;
            LogMessage("MSGMNG", "", "[Scanner] Scanning range: " + range + " (" + std::to_string(rangeTotal) + " IPs, port=" + std::to_string(port) + ")");

            for (uint32_t ip = start; ip <= end && scanning_; ++ip) {
                std::string ipStr = ipmsg::Uint32ToIP(ip);

                // Snapshot per IP: the user may edit nickname/group mid-scan.
                const UserInfo local = LocalUserSnapshot();

                // Skip our own IP
                if (ipStr == local.ipAddress) {
                    continue;
                }

                // Send BR_ENTRY to this IP
                std::string body = local.nickName;
                std::string extra = local.groupName;
                uint64_t pktNo = MakePacketNo();
                auto msg = MakeMsg(pktNo,
                    IPMSG_BR_ENTRY | IPMSG_CAPUTF8OPT,
                    body, extra);

                LogMessage("MSGMNG", "", "[Scanner] Sending BR_ENTRY to " + ipStr + ":" + std::to_string(port) + " (packet=" + std::to_string(pktNo) + ")");
                UdpSend(ipStr, port, msg);

                // Progress callback (every 10 IPs or at end of range)
                if (scanProgressCallback_) {
                    scanProgressCallback_(0, 0, scanFoundCount_.load());
                }

                // Small delay to avoid flooding network
                std::this_thread::sleep_for(std::chrono::milliseconds(delayMs));
            }
        }

        // Grace period: wait for async responses (up to 3 seconds)
        LogMessage("MSGMNG", "", "[Scanner] Sent all BR_ENTRY, waiting for responses...");
        const int gracePeriodMs = 3000;
        auto graceStart = std::chrono::steady_clock::now();
        while (scanning_ && std::chrono::steady_clock::now() - graceStart < std::chrono::milliseconds(gracePeriodMs)) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
            if (scanProgressCallback_) {
                scanProgressCallback_(0, 0, scanFoundCount_.load());
            }
        }

        // Scan complete
        scanning_ = false;
        if (scanCompleteCallback_) {
            scanCompleteCallback_(scanFoundCount_.load());
        }
        LogMessage("MSGMNG", "", "[Scanner] Scan complete. Found: " + std::to_string(scanFoundCount_.load()));
    });

    return true;
}

void MsgMng::CancelScan() {
    bool wasScanning = scanning_.exchange(false);
    // Always reap the thread, even if the scan already finished on its own,
    // so scanThread_ is never left joinable.
    if (scanThread_.joinable()) {
        scanThread_.join();
    }
    if (wasScanning) {
        LogMessage("MSGMNG", "", "[Scanner] Scan cancelled by user");
    }
}

} // namespace ipmsg
