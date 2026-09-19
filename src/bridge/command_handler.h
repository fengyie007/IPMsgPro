#pragma once
// ============================================================================
// Bridge Command Handler
// Registers all Bridge commands for frontend-backend communication
// ============================================================================

#include <tauricpp/bridge.hpp>
#include <tauricpp/window.hpp>
#include "ipmsg/msgmng.h"
#include "database/message_db.h"
#include "file/file_transfer.h"
#include <map>
#include <set>
#include <chrono>
#include <memory>
#include <mutex>
#include <string>

namespace ipmsg {

class CommandHandler {
public:
    /// Get singleton instance
    static CommandHandler& Instance();

    /// Initialize with references to core components
    void Init(tauricpp::Bridge& bridge,
              MsgMng& msgMng,
              MessageDB& msgDb,
              FileTransferManager& fileTransfer);

    /// Set main window handle (call after window is created)
    void SetNativeWindowHandle(void* hwnd);

    /// Set window reference for window operations (show/hide/close)
    void SetWindow(tauricpp::Window* window);

    /// Get the minimize behavior setting
    std::string GetMinimizeBehavior() const { return minimizeBehavior_; }

    /// Get the notification sound setting
    bool GetNotificationSound() const { return notificationSound_; }

    /// Register all Bridge commands
    void RegisterAllCommands();

    /// Setup event forwarding (IPMsg events -> Bridge events)
    void SetupEventForwarding();

private:
    CommandHandler() = default;

    // --- User Commands ---
    nlohmann::json HandleUserDiscover(const nlohmann::json& args);
    nlohmann::json HandleUserList(const nlohmann::json& args);
    nlohmann::json HandleUserStatus(const nlohmann::json& args);
    nlohmann::json HandleUserLocal(const nlohmann::json& args);

    // --- Message Commands ---
    nlohmann::json HandleMessageSend(const nlohmann::json& args);

    // --- File Commands ---
    nlohmann::json HandleFileSend(const nlohmann::json& args);
    nlohmann::json HandleFileInfo(const nlohmann::json& args);
    nlohmann::json HandleFileSaveTemp(const nlohmann::json& args);
    nlohmann::json HandleFileAccept(const nlohmann::json& args);
    nlohmann::json HandleFileReject(const nlohmann::json& args);
    nlohmann::json HandleFileOpenFolder(const nlohmann::json& args);

    // --- History Commands ---
    nlohmann::json HandleHistoryGet(const nlohmann::json& args);
    nlohmann::json HandleHistorySearch(const nlohmann::json& args);
    nlohmann::json HandleHistoryClear(const nlohmann::json& args);
    nlohmann::json HandleHistoryGetRecent(const nlohmann::json& args);

    // --- Network Commands ---
    nlohmann::json HandleNetworkScan(const nlohmann::json& args);
    nlohmann::json HandleNetworkScanRange(const nlohmann::json& args);
    nlohmann::json HandleNetworkScanCancel(const nlohmann::json& args);

    // --- Config Commands ---
    nlohmann::json HandleConfigSet(const nlohmann::json& args);
    nlohmann::json HandleConfigLoaded(const nlohmann::json& args);
    nlohmann::json HandleFrontendError(const nlohmann::json& args);

    // --- Dialog Commands ---
    nlohmann::json HandleDialogPickFolder(const nlohmann::json& args);
    nlohmann::json HandleDialogOpen(const nlohmann::json& args);
    nlohmann::json HandleDialogSave(const nlohmann::json& args);
    nlohmann::json HandleShellOpen(const nlohmann::json& args);

    // --- Screenshot Commands ---
    nlohmann::json HandleScreenshotCapture(const nlohmann::json& args);
    nlohmann::json HandleWindowMaximize(const nlohmann::json& args);
    nlohmann::json HandleWindowRestore(const nlohmann::json& args);
    nlohmann::json HandleWindowSetAlwaysOnTop(const nlohmann::json& args);

    // --- File Commands (extra) ---
    nlohmann::json HandleFileSaveData(const nlohmann::json& args);

    // --- Helper: Convert UserInfo to JSON ---
    static nlohmann::json UserToJson(const UserInfo& user);

    // --- Helper: Find user by IP or key ---
    std::optional<UserInfo> FindUserFromArgs(const nlohmann::json& args);

public:
    /// Get the effective data directory (custom or default)
    std::string GetDataDir() const;

private:
    /// Write a protocol diagnostic blob to <dataDir>\debug\<fileName>.
    /// No-op unless the DEBUG log level is active (--verbose).
    void DumpDebugFile(const std::string& fileName, const std::string& data) const;
    tauricpp::Bridge* bridge_ = nullptr;
    MsgMng* msgMng_ = nullptr;
    MessageDB* msgDb_ = nullptr;
    FileTransferManager* fileTransfer_ = nullptr;
    tauricpp::Window* window_ = nullptr;  // Window reference for show/hide/close
    void* hwnd_ = nullptr;  // Main window handle for dialogs (cast to HWND in cpp)
    std::string dataDir_;   // Custom data directory (empty = use default)
    std::string minimizeBehavior_ = "taskbar";  // "taskbar" or "tray"
    bool notificationSound_ = true;  // play notification sound on new messages

    // Text messages sent with IPMSG_SENDCHECKOPT that have not been acknowledged
    // yet: SENDMSG packetNo -> database message id. Filled on the UI thread by
    // HandleMessageSend, consumed on the UDP receive thread when the peer's
    // RECVMSG arrives. Entries are pruned after kPendingAckMaxAgeSec.
    struct PendingAck {
        std::string messageId;
        std::chrono::steady_clock::time_point sentAt;
    };
    std::map<uint64_t, PendingAck> pendingAcks_;
    std::mutex pendingAcksMutex_;
    static constexpr int kPendingAckMaxAgeSec = 600;
    void RegisterPendingAck(uint64_t packetNo, const std::string& messageId);
    std::string TakePendingAck(uint64_t packetNo);

    // --- FeiQ inline screenshot (custom fragmented image protocol) ---
    struct FeiQScreenshot {
        std::string id;
        std::string senderKey;
        uint64_t refPacketNo = 0;
        int totalSize = 0;
        int fragCount = 0;
        bool hasRef = false;
        ipmsg::UserInfo sender;                // captured from the reference message
        std::map<int, std::string> frags;      // fragIndex -> chunk bytes (leading 0x00 stripped)
    };
    std::map<std::string, FeiQScreenshot> feiqShots_;
    // Set of screenshot ids we have already reassembled + emitted. FeiQ has no
    // reliable ack for inline screenshots and periodically re-sends the whole
    // fragment set (observed ~every 30s) until it gives up. Any fragment whose
    // id is in this set is dropped outright so we never reassemble / re-emit the
    // same image. (FeiQ ids are random 8-hex values, so a genuine collision with
    // a future, distinct screenshot is negligible.) Kept for the whole session;
    // the volume of screenshots is tiny.
    std::set<std::string> emittedIds_;
    std::mutex feiqMutex_;
    void HandleFeiQScreenshotReference(const ipmsg::MsgBuf& msg, const std::string& body);
    bool HandleFeiQScreenshotFragment(const ipmsg::MsgBuf& msg);
    void FinalizeFeiQScreenshot(const std::string& key);
};

/// Return the current user's Downloads folder, e.g. C:\Users\<user>\Downloads.
/// Falls back to USERPROFILE\Downloads if the known-folder lookup fails.
std::string GetUserDownloadsDir();

} // namespace ipmsg
