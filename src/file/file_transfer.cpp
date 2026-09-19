// ============================================================================
// File Transfer Manager Implementation
// TCP-based file sending/receiving for IPMsg protocol
// Protocol reference: ipmsg-master and feiq (FeiQ) source code
// ============================================================================

#include "file_transfer.h"
#include "ipmsg/protocol.h"
#include "logger.h"
#include <fstream>
#include <sstream>
#include <chrono>
#include <random>
#include <filesystem>
#include <iomanip>
#include <algorithm>

#ifdef _WIN32
#include <windows.h>
#include <shlobj.h>
#endif

namespace ipmsg {

namespace fs = std::filesystem;

namespace {

// Decrements the active worker counter when a detached worker thread exits
// (by any path, including exceptions).
struct WorkerExit {
    std::atomic<int>& counter;
    ~WorkerExit() { --counter; }
};

// Socket timeouts for the accepted (sending) side. Without them a peer that
// connects and then stalls would pin a worker thread forever, and Shutdown()
// could never reap it.
constexpr int kRequestRecvTimeoutMs = 10000;
constexpr int kSendTimeoutMs = 30000;

void SetSocketTimeouts(SOCKET s, int recvMs, int sendMs) {
    if (recvMs > 0) {
        setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, reinterpret_cast<const char*>(&recvMs), sizeof(recvMs));
    }
    if (sendMs > 0) {
        setsockopt(s, SOL_SOCKET, SO_SNDTIMEO, reinterpret_cast<const char*>(&sendMs), sizeof(sendMs));
    }
}

}  // namespace

// Convert a UTF-8 path string to a wide string. The backend receives paths from
// the frontend as UTF-8 (which may contain Chinese user names / file names, e.g.
// "C:\\Users\\冯波\\Downloads"). Building std::filesystem::path directly from a
// UTF-8 std::string via the deprecated u8path() mis-handles the encoding on MSVC
// and triggers "No mapping for the Unicode character exists in the target
// multi-byte code page" when the path is opened. Going through an explicit
// UTF-16 (std::wstring) keeps everything on the wide API path (CreateFileW etc.).
static std::wstring Utf8ToWide(const std::string& s) {
    if (s.empty()) return {};
    int len = MultiByteToWideChar(CP_UTF8, 0, s.c_str(), static_cast<int>(s.size()), nullptr, 0);
    if (len <= 0) return {};
    std::wstring w(len, 0);
    MultiByteToWideChar(CP_UTF8, 0, s.c_str(), static_cast<int>(s.size()), &w[0], len);
    return w;
}

static fs::path PathFromUtf8(const std::string& s) {
    return fs::path(Utf8ToWide(s));
}

FileTransferManager::FileTransferManager() = default;

FileTransferManager::~FileTransferManager() {
    Shutdown();
}

bool FileTransferManager::Init(int tcpPort) {
    if (ready_) return true;

    // IPMsg uses the same port number for UDP messaging and TCP file transfer.
    // Peers connect to the port our UDP packets came from, so binding any
    // other TCP port would make every outgoing file transfer fail silently.
    tcpPort_ = (tcpPort == 0) ? IPMSG_DEFAULT_PORT : tcpPort;

    tcpListenSocket_ = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (tcpListenSocket_ == INVALID_SOCKET) {
        LogMessage("FILE_XFER", "ERROR", "[FileTransfer] Failed to create TCP socket: " + std::to_string(WSAGetLastError()));
        return false;
    }

    int optval = 1;
    setsockopt(tcpListenSocket_, SOL_SOCKET, SO_REUSEADDR,
               reinterpret_cast<const char*>(&optval), sizeof(optval));

    sockaddr_in addr = {};
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = INADDR_ANY;
    addr.sin_port = htons(static_cast<u_short>(tcpPort_));

    if (bind(tcpListenSocket_, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) == SOCKET_ERROR) {
        int bindErr = WSAGetLastError();
        LogMessage("FILE_XFER", "ERROR", "[FileTransfer] Failed to bind TCP port " + std::to_string(tcpPort_) +
                   ": " + std::to_string(bindErr) + " (file transfer disabled; is another IPMsg client running?)");
        closesocket(tcpListenSocket_);
        tcpListenSocket_ = INVALID_SOCKET;
        return false;
    }

    // Listen for connections
    if (listen(tcpListenSocket_, SOMAXCONN) == SOCKET_ERROR) {
        LogMessage("FILE_XFER", "ERROR", "[FileTransfer] Failed to listen: " + std::to_string(WSAGetLastError()));
        closesocket(tcpListenSocket_);
        tcpListenSocket_ = INVALID_SOCKET;
        return false;
    }

    LogMessage("FILE_XFER", "", "[FileTransfer] TCP server listening on port " + std::to_string(tcpPort_));

    // Start accept thread
    running_ = true;
    acceptThread_ = std::thread(&FileTransferManager::AcceptThreadFunc, this);

    ready_ = true;
    return true;
}

void FileTransferManager::Shutdown() {
    if (!ready_) return;

    running_ = false;

    // Close listening socket to unblock accept
    if (tcpListenSocket_ != INVALID_SOCKET) {
        closesocket(tcpListenSocket_);
        tcpListenSocket_ = INVALID_SOCKET;
    }

    // Wait for accept thread
    if (acceptThread_.joinable()) {
        acceptThread_.join();
    }

    // Cancel all active transfers
    {
        std::lock_guard<std::mutex> lock(transfersMutex_);
        for (auto& [id, transfer] : transfers_) {
            if (transfer.status == TransferStatus::Transferring ||
                transfer.status == TransferStatus::Pending) {
                transfer.status = TransferStatus::Cancelled;
            }
        }
    }

    // Wait (bounded) for the detached send/recv workers to notice the
    // cancellation and exit. Workers check the status once per chunk and all
    // sockets carry timeouts, so this normally completes within milliseconds.
    const int kMaxWaitMs = 5000;
    const int kStepMs = 20;
    int waitedMs = 0;
    while (activeWorkers_.load() > 0 && waitedMs < kMaxWaitMs) {
        std::this_thread::sleep_for(std::chrono::milliseconds(kStepMs));
        waitedMs += kStepMs;
    }
    if (activeWorkers_.load() > 0) {
        LogMessage("FILE_XFER", "WARN", "[FileTransfer] " + std::to_string(activeWorkers_.load()) +
                   " worker thread(s) still running after " + std::to_string(kMaxWaitMs) + "ms");
    }

    ready_ = false;
    LogMessage("FILE_XFER", "", "[FileTransfer] Shutdown complete");
}

void FileTransferManager::AcceptThreadFunc() {
    while (running_) {
        sockaddr_in clientAddr = {};
        int addrLen = sizeof(clientAddr);

        SOCKET clientSocket = accept(tcpListenSocket_, 
                                     reinterpret_cast<sockaddr*>(&clientAddr), 
                                     &addrLen);

        if (clientSocket == INVALID_SOCKET) {
            if (running_) {
                LogMessage("FILE_XFER", "", "[FileTransfer] Accept failed: " + std::to_string(WSAGetLastError()));
            }
            continue;
        }

        LogMessage("FILE_XFER", "", "[FileTransfer] Incoming connection from " + std::string(inet_ntoa(clientAddr.sin_addr)) + ":" + std::to_string(ntohs(clientAddr.sin_port)));

        // A stalled peer must not pin this worker forever (see Shutdown()).
        SetSocketTimeouts(clientSocket, kRequestRecvTimeoutMs, kSendTimeoutMs);

        // Handle connection in a new thread
        ++activeWorkers_;
        try {
            std::thread([this, clientSocket, clientAddr]() {
                WorkerExit exitGuard{activeWorkers_};
                try {
                    HandleClientConnection(clientSocket, clientAddr);
                } catch (const std::exception& e) {
                    LogMessage("FILE_XFER", "", "UNCAUGHT std::exception in HandleClientConnection from " +
                                     std::string(inet_ntoa(clientAddr.sin_addr)) + ": " + e.what());
                } catch (...) {
                    LogMessage("FILE_XFER", "", "UNCAUGHT unknown exception in HandleClientConnection from " +
                                     std::string(inet_ntoa(clientAddr.sin_addr)));
                }
            }).detach();
        } catch (const std::exception& e) {
            --activeWorkers_;
            closesocket(clientSocket);
            LogMessage("FILE_XFER", "ERROR", "[FileTransfer] Failed to start connection thread: " + std::string(e.what()));
        }
    }
}

void FileTransferManager::HandleClientConnection(SOCKET clientSocket, 
                                                  const sockaddr_in& clientAddr) {
    char buffer[4096] = {};
    int received = recv(clientSocket, buffer, sizeof(buffer) - 1, 0);

    if (received <= 0) {
        closesocket(clientSocket);
        return;
    }

    std::string request(buffer, received);
    LogMessage("FILE_XFER", "", "=== RECEIVED TCP REQUEST FROM " + std::string(inet_ntoa(clientAddr.sin_addr)) + ":" + std::to_string(ntohs(clientAddr.sin_port)) + " ===");
    LogMessage("FILE_XFER", "", "Request length: " + std::to_string(received) + " bytes");
    
    // Print raw hex dump
    std::ostringstream hexDump;
    for (int i = 0; i < received; i++) {
        if (i % 16 == 0) hexDump << "\n0x" << std::hex << std::setfill('0') << std::setw(4) << i << ": ";
        hexDump << std::hex << std::setfill('0') << std::setw(2) << (unsigned int)(unsigned char)buffer[i] << " ";
        if (i % 16 == 15) {
            hexDump << " ";
            for (int j = i - 15; j <= i; j++) {
                if (buffer[j] >= 32 && buffer[j] <= 126) {
                    hexDump << buffer[j];
                } else {
                    hexDump << ".";
                }
            }
        }
    }
    LogMessage("FILE_XFER", "", "Raw hex dump:" + hexDump.str());
    
    // Print as string (replace non-printable chars)
    std::string printableStr;
    for (int i = 0; i < received; i++) {
        if (buffer[i] >= 32 && buffer[i] <= 126) {
            printableStr += buffer[i];
        } else if (buffer[i] == '\0') {
            printableStr += "\\0";
        } else if (buffer[i] == '\n') {
            printableStr += "\\n";
        } else if (buffer[i] == '\r') {
            printableStr += "\\r";
        } else {
            printableStr += "\\x" + std::to_string((unsigned int)(unsigned char)buffer[i]);
        }
    }
    LogMessage("FILE_XFER", "", "Printable string: " + printableStr);
    LogMessage("FILE_XFER", "", "=== END REQUEST ===");

    // Parse IPMsg protocol header
    // Format: ver:packetNo:userName:hostName:command:body[\0extra]
    std::istringstream iss(request);
    std::string verStr, pktNoStr, userName, hostName, cmdStr, body;

    if (!std::getline(iss, verStr, ':') ||
        !std::getline(iss, pktNoStr, ':') ||
        !std::getline(iss, userName, ':') ||
        !std::getline(iss, hostName, ':') ||
        !std::getline(iss, cmdStr, ':')) {
        LogMessage("FILE_XFER", "", "[FileTransfer] Failed to parse IPMsg header");
        closesocket(clientSocket);
        return;
    }

    // Read the rest as body (may contain ':')
    std::getline(iss, body, '\0');

    uint32_t command = 0;
    try { command = std::stoul(cmdStr, nullptr, 10); } catch (...) {}

    // Check command type
    uint32_t cmdMode = command & 0x000000ff;  // Lower byte = command mode
    LogMessage("FILE_XFER", "", "Parsed command: " + std::to_string(command) + ", mode=0x" + 
                     ([](uint32_t v)->std::string{std::ostringstream o;o<<std::hex<<v;return o.str();})(cmdMode));

    if (cmdMode != IPMSG_GETFILEDATA && cmdMode != IPMSG_GETDIRFILES) {
        LogMessage("FILE_XFER", "", "Unknown command mode: 0x" + 
                         ([](uint32_t v)->std::string{std::ostringstream o;o<<std::hex<<v;return o.str();})(cmdMode) +
                         ", closing connection");
        closesocket(clientSocket);
        return;
    }

    // Find the extra data
    // FeiQ format: no \0 separator, extra data directly follows command field
    // Standard IPMsg format: \0 separates body and extra
    std::string extraData;
    const char* extraPtr = nullptr;
    for (int i = 0; i < received - 1; i++) {
        if (buffer[i] == '\0') {
            extraPtr = buffer + i + 1;
            break;
        }
    }

    if (extraPtr && *extraPtr != '\0') {
        // Standard IPMsg format: extra data after \0
        extraData = std::string(extraPtr);
        LogMessage("FILE_XFER", "", "Extra data from \\0 separator: " + extraData);
    } else {
        // FeiQ format: no \0 separator, extra data is the body itself
        // body already contains "packetNo(hex):fileId(hex):offset(hex):"
        extraData = body;
        LogMessage("FILE_XFER", "", "Extra data from body (no \\0 separator): " + extraData);
    }

    if (extraData.empty()) {
        LogMessage("FILE_XFER", "", "No extra data in request, closing connection");
        closesocket(clientSocket);
        return;
    }

    // Parse extra: "packetNo(hex):fileId(hex):offset(hex):"
    LogMessage("FILE_XFER", "", "Parsing extra data: " + extraData);
    std::istringstream extraIss(extraData);
    std::string reqPktNoStr, fileIdStr, offsetStr;

    uint64_t reqPacketNo = 0;
    int fileId = 0;
    int64_t offset = 0;

    if (std::getline(extraIss, reqPktNoStr, ':')) {
        try { reqPacketNo = std::stoull(reqPktNoStr, nullptr, 16); } catch (...) {}
    }
    if (std::getline(extraIss, fileIdStr, ':')) {
        try { fileId = std::stoi(fileIdStr, nullptr, 16); } catch (...) {}
    }
    if (std::getline(extraIss, offsetStr, ':')) {
        try { offset = std::stoll(offsetStr, nullptr, 16); } catch (...) {}
    }

    LogMessage("FILE_XFER", "", "GETFILEDATA parsed: reqPacketNo=" + std::to_string(reqPacketNo) + 
                     " (0x" + reqPktNoStr + "), fileId=" + std::to_string(fileId) + 
                     " (0x" + fileIdStr + "), offset=" + std::to_string(offset));

    // Debug: print all registered file infos for matching
    {
        std::lock_guard<std::mutex> lock(fileInfoMutex_);
        for (const auto& [tid, fi] : fileInfoRegistry_) {
            LogMessage("FILE_XFER", "", "  Registered: transferId=" + tid + 
                         ", packetNo=" + std::to_string(fi.packetNo) +
                         ", fileId=" + std::to_string(fi.fileId));
        }
    }

    // Find file info by matching both packetNo and fileId (reference: Feiq onTcpClientConnected)
    // The GETFILEDATA request's extra field contains the original SENDMSG's packetNo and fileId
    std::string filePath;
    std::string matchedTransferId;
    int64_t fileSize = 0;

    {
        std::lock_guard<std::mutex> lock(fileInfoMutex_);
        for (const auto& [transferId, fileInfo] : fileInfoRegistry_) {
            // Match both packetNo (from original SENDMSG) and fileId
            if (fileInfo.packetNo == reqPacketNo && fileInfo.fileId == fileId) {
                LogMessage("FILE_XFER", "", "Match found: transferId=" + transferId +
                                 ", packetNo=" + std::to_string(fileInfo.packetNo) +
                                 ", fileId=" + std::to_string(fileInfo.fileId));
                // Find the transfer
                std::lock_guard<std::mutex> tlock(transfersMutex_);
                auto it = transfers_.find(transferId);
                if (it != transfers_.end()) {
                    LogMessage("FILE_XFER", "", "Transfer found: isSending=" + std::to_string(it->second.isSending) +
                                     ", localPath=" + it->second.localPath);
                    if (it->second.isSending) {
                        filePath = it->second.localPath;
                        fileSize = fileInfo.fileSize;
                        matchedTransferId = transferId;
                        break;
                    }
                } else {
                    LogMessage("FILE_XFER", "", "Transfer NOT found in transfers_ map!");
                }
            }
        }
    }

    // Strict matching: only match by packetNo+fileId, no fallback
    // Fallback by fileId alone is dangerous since fileId=0 for all files

    LogMessage("FILE_XFER", "", "File path resolved: '" + filePath + "', matchedTransferId=" + matchedTransferId +
                     ", exists=" + (filePath.empty() ? "N/A(empty)" : (fs::exists(PathFromUtf8(filePath)) ? "yes" : "NO")));

    if (filePath.empty() || !fs::exists(PathFromUtf8(filePath))) {
        LogMessage("FILE_XFER", "", "File NOT found for GETFILEDATA! reqPacketNo=" + std::to_string(reqPacketNo) +
                         ", fileId=" + std::to_string(fileId) + ", filePath='" + filePath + "'");
        closesocket(clientSocket);
        return;
    }

    LogMessage("FILE_XFER", "", "Using matchedTransferId=" + matchedTransferId + " for SendFileThread");

    try {
        LogMessage("FILE_XFER", "", ">> SendFileThread enter: transferId=" + matchedTransferId +
                         ", filePath=" + filePath + ", fileSize=" + std::to_string(fileSize) +
                         ", offset=" + std::to_string(offset));
        SendFileThread(matchedTransferId, clientSocket, filePath, fileSize, offset);
        LogMessage("FILE_XFER", "", "<< SendFileThread exit OK: transferId=" + matchedTransferId);
    } catch (const std::exception& e) {
        LogMessage("FILE_XFER", "", "EXCEPTION in SendFileThread: " + std::string(e.what()) +
                         " transferId=" + matchedTransferId + " filePath=" + filePath);
    } catch (...) {
        LogMessage("FILE_XFER", "", "UNKNOWN EXCEPTION in SendFileThread transferId=" + matchedTransferId +
                         " filePath=" + filePath);
    }
}

std::string FileTransferManager::StartSendFile(const std::string& targetIp, int targetPort,
                                                const std::string& filePath,
                                                const std::string& toUser) {
    if (!ready_) return "";

    // Check if file exists
    if (!fs::exists(PathFromUtf8(filePath))) {
        LogMessage("FILE_XFER", "", "[FileTransfer] File not found: " + filePath);
        return "";
    }

    // Generate transfer ID
    std::string transferId = GenerateTransferId();

    // Get file info (use wide-aware path so Chinese paths resolve correctly)
    fs::path path(PathFromUtf8(filePath));
    // ★ 修复：使用 u8string() 获取 UTF-8 文件名，而非 string()（后者在 Windows 上用本地代码页 GBK，
    // 会产生非法 UTF-8，导致 Bridge::Emit 的 JSON dump 抛异常、进度事件无法送达前端）
    std::string fileName = path.filename().u8string();

    // Strip timestamp prefix from temp filename (format: "{timestamp}_{original_name}")
    // The temp file is created by HandleFileSaveTemp with a timestamp prefix
    {
        size_t underscorePos = fileName.find('_');
        if (underscorePos != std::string::npos && underscorePos > 0) {
            std::string prefix = fileName.substr(0, underscorePos);
            bool isAllDigits = !prefix.empty() && std::all_of(prefix.begin(), prefix.end(), ::isdigit);
            if (isAllDigits && prefix.length() >= 9) {
                // Looks like a Unix timestamp prefix, strip it
                fileName = fileName.substr(underscorePos + 1);
            }
        }
    }

    int64_t fileSize = fs::file_size(PathFromUtf8(filePath));

    // Create transfer record
    TransferProgress transfer;
    transfer.transferId = transferId;
    transfer.filename = fileName;
    transfer.fileSize = fileSize;
    transfer.transferred = 0;
    transfer.status = TransferStatus::Pending;
    transfer.fromUser = "local";
    transfer.toUser = toUser;
    transfer.localPath = filePath;
    transfer.isSending = true;

    {
        std::lock_guard<std::mutex> lock(transfersMutex_);
        transfers_[transferId] = transfer;
    }

    // Register file info - use packetNo as fileId (matching IPMsg protocol)
    FileInfo fileInfo;
    fileInfo.fileId = 0;  // IPMsg file serial number starts from 0
    fileInfo.fileName = fileName;
    fileInfo.fileSize = fileSize;
    // Use Unix time_t (seconds since epoch) as mtime, matching FeiQ/IPMsg format
    fileInfo.modifyTime = static_cast<int64_t>(std::time(nullptr));
    fileInfo.fileAttr = 1;  // IPMSG_FILE_REGULAR

    {
        std::lock_guard<std::mutex> lock(fileInfoMutex_);
        fileInfoRegistry_[transferId] = fileInfo;
    }

    LogMessage("FILE_XFER", "", "[FileTransfer] Started sending file: " + fileName + " (" + std::to_string(fileSize) + " bytes) to " + toUser);

    return transferId;
}

void FileTransferManager::SendFileThread(const std::string& transferId, SOCKET clientSocket,
                                          const std::string& filePath, int64_t fileSize,
                                          int64_t offset) {
    LogMessage("FILE_XFER", "", "SendFileThread started: transferId=" + transferId + 
                     ", filePath=" + filePath + ", fileSize=" + std::to_string(fileSize) +
                     ", offset=" + std::to_string(offset));

    // Update status to transferring
    UpdateTransferProgress(transferId, 0, TransferStatus::Transferring);

    // Open file (wide-aware path so Chinese source paths work)
    std::ifstream file(PathFromUtf8(filePath), std::ios::binary);
    if (!file.is_open()) {
        LogMessage("FILE_XFER", "", "Failed to open file: " + filePath);
        UpdateTransferProgress(transferId, 0, TransferStatus::Failed);
        closesocket(clientSocket);
        return;
    }
    LogMessage("FILE_XFER", "", "File opened successfully: " + filePath);

    // Seek to offset if needed
    if (offset > 0) {
        file.seekg(offset, std::ios::beg);
    }

    const int bufferSize = 64 * 1024; // 64KB buffer
    std::vector<char> buffer(bufferSize);
    int64_t totalSent = offset;

    while (totalSent < fileSize) {
        // Stop on cancellation or manager shutdown
        bool cancelled = !running_;
        if (!cancelled) {
            std::lock_guard<std::mutex> lock(transfersMutex_);
            auto it = transfers_.find(transferId);
            if (it != transfers_.end() && it->second.status == TransferStatus::Cancelled) {
                cancelled = true;
            }
        }
        if (cancelled) {
            LogMessage("FILE_XFER", "", "[FileTransfer] Transfer cancelled: " + transferId);
            break;
        }

        // Read from file
        file.read(buffer.data(), bufferSize);
        std::streamsize bytesRead = file.gcount();

        if (bytesRead <= 0) {
            break;
        }

        // Send data
        int sent = send(clientSocket, buffer.data(), static_cast<int>(bytesRead), 0);
        if (sent == SOCKET_ERROR) {
            int err = WSAGetLastError();
            LogMessage("FILE_XFER", "", "Send failed with error: " + std::to_string(err));
            UpdateTransferProgress(transferId, totalSent, TransferStatus::Failed);
            closesocket(clientSocket);
            return;
        }

        totalSent += sent;

        // Update progress
        UpdateTransferProgress(transferId, totalSent, TransferStatus::Transferring);

        // Log progress every 1MB
        if (totalSent % (1024 * 1024) < bufferSize) {
            LogMessage("FILE_XFER", "", "Sent " + std::to_string(totalSent) + "/" + std::to_string(fileSize) +
                             " bytes (" + std::to_string(totalSent * 100 / fileSize) + "%)");
        }
    }

    closesocket(clientSocket);

    // Update final status
    if (totalSent >= fileSize) {
        UpdateTransferProgress(transferId, totalSent, TransferStatus::Completed);
        LogMessage("FILE_XFER", "", "File sent successfully: " + filePath + " (" + std::to_string(totalSent) + " bytes)");
    } else {
        UpdateTransferProgress(transferId, totalSent, TransferStatus::Failed);
        LogMessage("FILE_XFER", "", "File send incomplete: " + filePath + " (" + std::to_string(totalSent) + "/" + std::to_string(fileSize) + " bytes)");
    }
}

std::string FileTransferManager::StartRecvFile(const std::string& fromUserIp, int fromUserPort,
                                                const std::string& fileName, int64_t fileSize,
                                                const std::string& savePath,
                                                const std::string& fromUser,
                                                uint64_t origPacketNo, int origFileId) {
    LogMessage("FILE_XFER", "", "StartRecvFile called: fromUserIp=" + fromUserIp + ", fromUserPort=" + std::to_string(fromUserPort) + ", fileName=" + fileName + ", ready_=" + std::to_string(ready_));
    
    if (!ready_) {
        LogMessage("FILE_XFER", "", "StartRecvFile failed: ready_ is false!");
        return "";
    }

    // Generate transfer ID
    std::string transferId = GenerateTransferId();
    LogMessage("FILE_XFER", "", "Generated transferId: " + transferId);

    // Create transfer record
    TransferProgress transfer;
    transfer.transferId = transferId;
    transfer.filename = fileName;
    transfer.fileSize = fileSize;
    transfer.transferred = 0;
    transfer.status = TransferStatus::Pending;
    transfer.fromUser = fromUser;
    transfer.toUser = "local";
    transfer.localPath = savePath;  // For received files, localPath is where the file is saved
    transfer.isSending = false;

    {
        std::lock_guard<std::mutex> lock(transfersMutex_);
        transfers_[transferId] = transfer;
    }
    LogMessage("FILE_XFER", "", "Transfer record saved to map");

    // Start receive thread, passing original packetNo and fileId for GETFILEDATA request
    LogMessage("FILE_XFER", "", "Starting receive thread...");
    ++activeWorkers_;
    try {
        std::thread([this, transferId, fromUserIp, fromUserPort, savePath, fileSize,
                     origPacketNo, origFileId]() {
            WorkerExit exitGuard{activeWorkers_};
            try {
                RecvFileThread(transferId, fromUserIp, fromUserPort, savePath, fileSize,
                               origPacketNo, origFileId);
            } catch (const std::exception& e) {
                LogMessage("FILE_XFER", "", "EXCEPTION in RecvFileThread: " + std::string(e.what()) +
                                 " transferId=" + transferId + " savePath=" + savePath);
            } catch (...) {
                LogMessage("FILE_XFER", "", "UNKNOWN EXCEPTION in RecvFileThread transferId=" + transferId +
                                 " savePath=" + savePath);
            }
        }).detach();
        LogMessage("FILE_XFER", "", "Receive thread started successfully");
    } catch (const std::exception& e) {
        --activeWorkers_;
        LogMessage("FILE_XFER", "", "Failed to start receive thread: " + std::string(e.what()));
        return "";
    }

    LogMessage("FILE_XFER", "", "[FileTransfer] Started receiving file: " + fileName + " from " + fromUser);

    return transferId;
}

void FileTransferManager::RecvFileThread(const std::string& transferId, const std::string& fromIp,
                                          int fromPort, const std::string& savePath, int64_t fileSize,
                                          uint64_t origPacketNo, int origFileId) {
    LogMessage("FILE_XFER", "", "RecvFileThread started: transferId=" + transferId + ", fromIp=" + fromIp + ", fromPort=" + std::to_string(fromPort) + ", savePath=" + savePath + ", fileSize=" + std::to_string(fileSize) + ", origPacketNo=" + std::to_string(origPacketNo) + ", origFileId=" + std::to_string(origFileId));

    // Update status to transferring
    UpdateTransferProgress(transferId, 0, TransferStatus::Transferring);

    // Create directory if not exists (wide-aware path for Chinese directories)
    fs::path path(PathFromUtf8(savePath));
    fs::create_directories(path.parent_path());

    // Open file for writing (wide-aware path so Chinese save paths work)
    std::ofstream file(PathFromUtf8(savePath), std::ios::binary);
    if (!file.is_open()) {
        LogMessage("FILE_XFER", "", "Failed to create file: " + savePath);
        UpdateTransferProgress(transferId, 0, TransferStatus::Failed);
        return;
    }
    LogMessage("FILE_XFER", "", "File created: " + savePath);

    // Connect to sender's TCP server (same port as UDP per IPMsg protocol)
    LogMessage("FILE_XFER", "", "Creating TCP socket...");
    SOCKET sendSocket = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (sendSocket == INVALID_SOCKET) {
        LogMessage("FILE_XFER", "", "Failed to create socket");
        UpdateTransferProgress(transferId, 0, TransferStatus::Failed);
        return;
    }
    LogMessage("FILE_XFER", "", "Socket created successfully");

    sockaddr_in serverAddr = {};
    serverAddr.sin_family = AF_INET;
    serverAddr.sin_port = htons(static_cast<u_short>(fromPort));
    inet_pton(AF_INET, fromIp.c_str(), &serverAddr.sin_addr);

    LogMessage("FILE_XFER", "", "Connecting to " + fromIp + ":" + std::to_string(fromPort) + "...");
    if (connect(sendSocket, reinterpret_cast<sockaddr*>(&serverAddr), sizeof(serverAddr)) == SOCKET_ERROR) {
        int err = WSAGetLastError();
        LogMessage("FILE_XFER", "", "Failed to connect to sender " + fromIp + ":" + std::to_string(fromPort) + " (err=" + std::to_string(err) + ")");
        closesocket(sendSocket);
        UpdateTransferProgress(transferId, 0, TransferStatus::Failed);
        return;
    }
    LogMessage("FILE_XFER", "", "Connected to " + fromIp + ":" + std::to_string(fromPort));

    // Get local user info for the protocol header
    char localUserName[256] = {};
    char localHostName[256] = {};
    DWORD size;
    size = sizeof(localUserName);
    GetUserNameA(localUserName, &size);
    size = sizeof(localHostName);
    GetComputerNameA(localHostName, &size);

    // Generate a new packetNo for this GETFILEDATA request itself
    uint32_t newPktNo = static_cast<uint32_t>(
        std::chrono::system_clock::now().time_since_epoch().count() & 0xFFFFFFFF);

    // First, send RECVMSG via UDP to tell the sender we accept the file transfer
    // FeiQ expects RECVMSG (with decimal packetNo in body) before it will accept TCP GETFILEDATA
    LogMessage("FILE_XFER", "", "Sending RECVMSG to accept file transfer...");
    {
        SOCKET udpSocket = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
        if (udpSocket != INVALID_SOCKET) {
            // RECVMSG body must be the original SENDMSG's packetNo in DECIMAL
            // IMPORTANT: Use std::dec explicitly and use a fresh ostringstream to avoid hex contamination
            std::ostringstream ackMsg;
            ackMsg << std::dec << IPMSG_VERSION << ":" << std::dec << newPktNo << ":"
                   << localUserName << ":" << localHostName << ":"
                   << IPMSG_RECVMSG << ":" << std::dec << origPacketNo;
            
            sockaddr_in destAddr = {};
            destAddr.sin_family = AF_INET;
            destAddr.sin_port = htons(static_cast<u_short>(fromPort));
            inet_pton(AF_INET, fromIp.c_str(), &destAddr.sin_addr);
            
            std::string ackStr = ackMsg.str();
            int sendResult = sendto(udpSocket, ackStr.c_str(), static_cast<int>(ackStr.size()), 0,
                         reinterpret_cast<sockaddr*>(&destAddr), sizeof(destAddr));
            closesocket(udpSocket);
            LogMessage("FILE_XFER", "", "RECVMSG sent: " + ackStr + " (sendResult=" + std::to_string(sendResult) + ")");
        }
    }
    // Wait for the sender to process RECVMSG and be ready for TCP
    std::this_thread::sleep_for(std::chrono::milliseconds(500));

    // Build IPMsg GETFILEDATA request
    // FeiQ format: "ver:packetNo:userName:hostName:96:origPacketNo(hex):fileId(hex):offset(hex):"
    // NOTE: FeiQ does NOT use \0 separator between header and extra!
    // The extra data follows directly after the command field's colon separator.
    std::ostringstream requestOs;
    requestOs << IPMSG_VERSION << ":" << newPktNo << ":"
              << localUserName << ":" << localHostName << ":"
              << IPMSG_GETFILEDATA << ":"
              << std::hex << origPacketNo << ":" << origFileId << ":0:";

    std::string request = requestOs.str();

    std::ostringstream hexCmd;
    hexCmd << std::hex << IPMSG_GETFILEDATA;
    LogMessage("FILE_XFER", "", "GETFILEDATA request format: ver=" + std::to_string(IPMSG_VERSION) + 
                      ", newPktNo=" + std::to_string(newPktNo) +
                      ", command=" + std::to_string(IPMSG_GETFILEDATA) + " (0x" + hexCmd.str() + ")" +
                      ", extra=" + std::to_string(origPacketNo) + ":" + std::to_string(origFileId) + ":0");
    
    // Print raw bytes of the request for debugging
    LogMessage("FILE_XFER", "", "=== GETFILEDATA REQUEST RAW BYTES ===");
    LogMessage("FILE_XFER", "", "Request length: " + std::to_string(request.size()) + " bytes");
    std::ostringstream rawHex;
    std::ostringstream rawAscii;
    for (size_t i = 0; i < request.size(); i++) {
        rawHex << std::hex << std::setfill('0') << std::setw(2) << (unsigned int)(unsigned char)request[i] << " ";
        if (request[i] >= 32 && request[i] <= 126) {
            rawAscii << request[i];
        } else if (request[i] == '\0') {
            rawAscii << "\\0";
        } else if (request[i] == '\n') {
            rawAscii << "\\n";
        } else {
            rawAscii << ".";
        }
        if (i % 16 == 15) {
            LogMessage("FILE_XFER", "", "0x" + std::to_string(i - 15) + ": " + rawHex.str() + " | " + rawAscii.str());
            rawHex.str("");
            rawAscii.str("");
        }
    }
    if (!rawHex.str().empty()) {
        LogMessage("FILE_XFER", "", "0x" + std::to_string(request.size() - (request.size() % 16)) + ": " + rawHex.str() + " | " + rawAscii.str());
    }
    LogMessage("FILE_XFER", "", "=== END REQUEST ===");

    if (::send(sendSocket, request.data(), static_cast<int>(request.size()), 0) == SOCKET_ERROR) {
        int err = WSAGetLastError();
        LogMessage("FILE_XFER", "", "Failed to send GETFILEDATA request (err=" + std::to_string(err) + ")");
        closesocket(sendSocket);
        UpdateTransferProgress(transferId, 0, TransferStatus::Failed);
        return;
    }
    LogMessage("FILE_XFER", "", "GETFILEDATA request sent successfully");

    // Set TCP_NODELAY to disable Nagle's algorithm (important for file transfer)
    int nodelay = 1;
    setsockopt(sendSocket, IPPROTO_TCP, TCP_NODELAY, reinterpret_cast<const char*>(&nodelay), sizeof(nodelay));
    LogMessage("FILE_XFER", "", "TCP_NODELAY set");

    // Receive file data
    const int bufferSize = 64 * 1024; // 64KB buffer
    std::vector<char> buffer(bufferSize);
    int64_t totalReceived = 0;
    int receiveTimeout = 5000; // 5 second timeout
    setsockopt(sendSocket, SOL_SOCKET, SO_RCVTIMEO, reinterpret_cast<const char*>(&receiveTimeout), sizeof(receiveTimeout));

    LogMessage("FILE_XFER", "", "Starting file data receive loop (timeout=" + std::to_string(receiveTimeout) + "ms)");

    while (fileSize <= 0 || totalReceived < fileSize) {
        // Stop on cancellation or manager shutdown
        bool cancelled = !running_;
        if (!cancelled) {
            std::lock_guard<std::mutex> lock(transfersMutex_);
            auto it = transfers_.find(transferId);
            if (it != transfers_.end() && it->second.status == TransferStatus::Cancelled) {
                cancelled = true;
            }
        }
        if (cancelled) {
            LogMessage("FILE_XFER", "", "Transfer cancelled");
            break;
        }

        int recvSize = (fileSize > 0) ?
            static_cast<int>((std::min)(static_cast<int64_t>(bufferSize), fileSize - totalReceived)) :
            bufferSize;

        int received = recv(sendSocket, buffer.data(), recvSize, 0);
        if (received <= 0) {
            int err = WSAGetLastError();
            if (err == WSAETIMEDOUT) {
                LogMessage("FILE_XFER", "", "recv() timed out after " + std::to_string(receiveTimeout) + "ms");
            } else if (err == 0) {
                LogMessage("FILE_XFER", "", "recv() returned 0 - connection closed by peer");
            } else {
                LogMessage("FILE_XFER", "", "recv() failed with error: " + std::to_string(err));
            }
            break;
        }

        file.write(buffer.data(), received);
        totalReceived += received;

        UpdateTransferProgress(transferId, totalReceived, TransferStatus::Transferring);

        LogMessage("FILE_XFER", "", "Received " + std::to_string(totalReceived) + "/" + std::to_string(fileSize) +
                          " bytes (" + std::to_string(fileSize > 0 ? totalReceived * 100 / fileSize : 0) + "%)");
    }

    closesocket(sendSocket);
    file.close();

    // Update final status
    if (fileSize <= 0 || totalReceived >= fileSize) {
        LogMessage("FILE_XFER", "", "File received successfully: " + savePath + " (" + std::to_string(totalReceived) + " bytes)");
        UpdateTransferProgress(transferId, totalReceived, TransferStatus::Completed);
    } else {
        LogMessage("FILE_XFER", "", "File receive failed: " + savePath + " (received " + std::to_string(totalReceived) + "/" + std::to_string(fileSize) + " bytes)");
        UpdateTransferProgress(transferId, totalReceived, TransferStatus::Failed);
        std::error_code ec;
        fs::remove(PathFromUtf8(savePath), ec);  // wide-aware; a narrow path would mangle Chinese names
    }
}

bool FileTransferManager::CancelTransfer(const std::string& transferId) {
    std::lock_guard<std::mutex> lock(transfersMutex_);
    auto it = transfers_.find(transferId);
    if (it != transfers_.end()) {
        it->second.status = TransferStatus::Cancelled;
        LogMessage("FILE_XFER", "", "[FileTransfer] Transfer cancelled: " + transferId);
        return true;
    }
    return false;
}

std::vector<TransferProgress> FileTransferManager::GetActiveTransfers() const {
    std::lock_guard<std::mutex> lock(transfersMutex_);
    std::vector<TransferProgress> result;
    for (const auto& [id, transfer] : transfers_) {
        result.push_back(transfer);
    }
    return result;
}

std::optional<TransferProgress> FileTransferManager::GetTransfer(const std::string& transferId) const {
    std::lock_guard<std::mutex> lock(transfersMutex_);
    auto it = transfers_.find(transferId);
    if (it != transfers_.end()) {
        return it->second;
    }
    return std::nullopt;
}

void FileTransferManager::RegisterFileInfo(const std::string& transferId, const FileInfo& fileInfo) {
    std::lock_guard<std::mutex> lock(fileInfoMutex_);
    fileInfoRegistry_[transferId] = fileInfo;
}

std::optional<FileInfo> FileTransferManager::GetFileInfo(const std::string& transferId) const {
    std::lock_guard<std::mutex> lock(fileInfoMutex_);
    auto it = fileInfoRegistry_.find(transferId);
    if (it != fileInfoRegistry_.end()) {
        return it->second;
    }
    return std::nullopt;
}

void FileTransferManager::UpdateTransferProgress(const std::string& transferId,
                                                  int64_t transferred,
                                                  TransferStatus status) {
    // Copy the record out and invoke the callback outside the lock, so a
    // callback that queries this manager (GetTransfer etc.) cannot deadlock.
    std::optional<TransferProgress> snapshot;
    {
        std::lock_guard<std::mutex> lock(transfersMutex_);
        auto it = transfers_.find(transferId);
        if (it != transfers_.end()) {
            it->second.transferred = transferred;
            it->second.status = status;
            snapshot = it->second;
        }
    }
    if (snapshot && onProgress_) {
        onProgress_(*snapshot);
    }
}

bool FileTransferManager::SendFileRequest(const std::string& targetIp, int targetPort,
                                           const std::string& transferId, int fileId) {
    // This is handled in RecvFileThread
    return true;
}

std::string FileTransferManager::GenerateTransferId() {
    auto now = std::chrono::system_clock::now().time_since_epoch().count();
    std::random_device rd;
    std::mt19937 gen(rd());
    std::uniform_int_distribution<> dis(100000, 999999);

    std::ostringstream oss;
    oss << std::hex << now << "_" << dis(gen);
    return oss.str();
}

} // namespace ipmsg
