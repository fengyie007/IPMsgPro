#pragma once
// ============================================================================
// File Transfer Manager
// TCP-based file sending/receiving for IPMsg protocol
// ============================================================================

#include <string>
#include <vector>
#include <functional>
#include <mutex>
#include <atomic>
#include <cstdint>
#include <map>
#include <thread>
#include <optional>

// WinSock2 must be included before Windows.h
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <WinSock2.h>
#include <WS2tcpip.h>

namespace ipmsg {

/// File transfer status
enum class TransferStatus {
    Pending = 0,
    Transferring = 1,
    Completed = 2,
    Failed = 3,
    Cancelled = 4
};

/// File transfer progress info
struct TransferProgress {
    std::string transferId;  // unique transfer ID
    std::string filename;    // file name
    int64_t fileSize = 0;    // total file size in bytes
    int64_t transferred = 0; // bytes transferred
    TransferStatus status = TransferStatus::Pending;
    std::string fromUser;    // sender user key
    std::string toUser;      // receiver user key
    std::string localPath;   // local file path (for sending)
    std::string remotePath;  // remote file path (for receiving)
    bool isSending = true;   // true = sending, false = receiving
};

/// File info for IPMsg file attachment
struct FileInfo {
    int fileId = 0;          // file index in message
    std::string fileName;    // file name
    int64_t fileSize = 0;    // file size in bytes
    int64_t modifyTime = 0;  // modification time (unix timestamp)
    int32_t fileAttr = 0;    // file attributes
    uint64_t packetNo = 0;   // original SENDMSG packetNo (for matching GETFILEDATA requests)
};

/// Callback for transfer progress updates
using TransferProgressCallback = std::function<void(const TransferProgress&)>;

/// File Transfer Manager
class FileTransferManager {
public:
    FileTransferManager();
    ~FileTransferManager();

    // Non-copyable
    FileTransferManager(const FileTransferManager&) = delete;
    FileTransferManager& operator=(const FileTransferManager&) = delete;

    /// Initialize the file transfer manager
    /// @param tcpPort TCP port for file transfer (0 = IPMsg default port).
    ///        Must equal the UDP port: peers connect to the port our BR_ENTRY
    ///        came from, so there is no fallback to other ports.
    bool Init(int tcpPort = 0);

    /// Shutdown. Cancels active transfers and waits (bounded) for the
    /// detached worker threads to exit before returning.
    void Shutdown();

    /// Check if initialized
    bool IsReady() const { return ready_; }

    /// Get TCP port
    int GetTcpPort() const { return tcpPort_; }

    // ---------- File Sending ----------

    /// Start sending a file to a user
    /// @return transfer ID on success, empty string on failure
    std::string StartSendFile(const std::string& targetIp, int targetPort,
                              const std::string& filePath,
                              const std::string& toUser);

    /// Send file data over TCP (called in separate thread)
    void SendFileThread(const std::string& transferId, SOCKET clientSocket,
                        const std::string& filePath, int64_t fileSize,
                        int64_t offset = 0);

    // ---------- File Receiving ----------

    /// Start receiving a file from a user
    /// @param origPacketNo The original SENDMSG packetNo (needed for GETFILEDATA request)
    /// @param origFileId The file ID from the attachment info (needed for GETFILEDATA request)
    /// @return transfer ID on success, empty string on failure
    std::string StartRecvFile(const std::string& fromUserIp, int fromUserPort,
                              const std::string& fileName, int64_t fileSize,
                              const std::string& savePath,
                              const std::string& fromUser,
                              uint64_t origPacketNo = 0, int origFileId = 0);

    /// Receive file data over TCP (called in separate thread)
    void RecvFileThread(const std::string& transferId, const std::string& fromIp,
                        int fromPort, const std::string& savePath, int64_t fileSize,
                        uint64_t origPacketNo, int origFileId);

    // ---------- Transfer Management ----------

    /// Cancel a file transfer
    bool CancelTransfer(const std::string& transferId);

    /// Get all active transfers
    std::vector<TransferProgress> GetActiveTransfers() const;

    /// Get transfer by ID
    std::optional<TransferProgress> GetTransfer(const std::string& transferId) const;

    // ---------- Callbacks ----------

    /// Set progress callback
    void SetProgressCallback(TransferProgressCallback cb) {
        onProgress_ = std::move(cb);
    }

    // ---------- File Info Management ----------

    /// Register file info for sending
    void RegisterFileInfo(const std::string& transferId, const FileInfo& fileInfo);

    /// Get file info by transfer ID
    std::optional<FileInfo> GetFileInfo(const std::string& transferId) const;

private:
    /// Generate a unique transfer ID
    static std::string GenerateTransferId();

    /// TCP server accept thread
    void AcceptThreadFunc();

    /// Handle incoming TCP connection
    void HandleClientConnection(SOCKET clientSocket, const sockaddr_in& clientAddr);

    /// Update transfer progress
    void UpdateTransferProgress(const std::string& transferId, int64_t transferred,
                                TransferStatus status);

private:
    SOCKET tcpListenSocket_ = INVALID_SOCKET;
    int tcpPort_ = 0;
    std::atomic<bool> ready_{false};
    std::atomic<bool> running_{false};

    // Accept thread
    std::thread acceptThread_;

    // Number of detached send/recv worker threads currently alive. Shutdown()
    // waits for this to reach zero so `this` is not destroyed under a worker.
    std::atomic<int> activeWorkers_{0};

    // Active transfers
    std::map<std::string, TransferProgress> transfers_;
    mutable std::mutex transfersMutex_;

    // File info registry (transferId -> FileInfo)
    std::map<std::string, FileInfo> fileInfoRegistry_;
    mutable std::mutex fileInfoMutex_;

    // Callbacks
    TransferProgressCallback onProgress_;
};

} // namespace ipmsg
