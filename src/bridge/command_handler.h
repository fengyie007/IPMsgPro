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
    nlohmann::json HandleMessageSendImage(const nlohmann::json& args);

    // --- File Commands ---
    nlohmann::json HandleFileSend(const nlohmann::json& args);
    nlohmann::json HandleFileInfo(const nlohmann::json& args);
    nlohmann::json HandleFileRecv(const nlohmann::json& args);
    nlohmann::json HandleFileSaveTemp(const nlohmann::json& args);
    nlohmann::json HandleFileAccept(const nlohmann::json& args);
    nlohmann::json HandleFileReject(const nlohmann::json& args);
    nlohmann::json HandleFileOpenFolder(const nlohmann::json& args);

    // --- History Commands ---
    nlohmann::json HandleHistoryGet(const nlohmann::json& args);
    nlohmann::json HandleHistorySearch(const nlohmann::json& args);
    nlohmann::json HandleHistoryClear(const nlohmann::json& args);

    // --- Network Commands ---
    nlohmann::json HandleNetworkScan(const nlohmann::json& args);

    // --- Config Commands ---
    nlohmann::json HandleConfigSet(const nlohmann::json& args);
    nlohmann::json HandleConfigLoaded(const nlohmann::json& args);

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
    nlohmann::json HandleFeiQScreenshotSend(const nlohmann::json& args);
    nlohmann::json HandleFeiQEchoScreenshot(const nlohmann::json& args);
    // Send a pre-built FeiQ inline-screenshot payload ("LZW!" + size + crc + LZW
    // stream bytes, exactly as FeiQ wires it) to `target` using the fragmented
    // inline protocol. dw/dh only size the inline reference placeholder. refCmd
    // / fragCmd are the wire command numbers (0 = use the default FeiQ values).
    bool SendFeiQShotPayload(const UserInfo& target, const std::string& payload,
                             int dw, int dh, uint32_t refCmd, uint32_t fragCmd);

    // --- File Commands (extra) ---
    nlohmann::json HandleFileSaveData(const nlohmann::json& args);

    // --- Helper: Convert UserInfo to JSON ---
    static nlohmann::json UserToJson(const UserInfo& user);

    // --- Helper: Find user by IP or key ---
    std::optional<UserInfo> FindUserFromArgs(const nlohmann::json& args);

    /// Get the effective data directory (custom or default)
    std::string GetDataDir() const;

private:
    tauricpp::Bridge* bridge_ = nullptr;
    MsgMng* msgMng_ = nullptr;
    MessageDB* msgDb_ = nullptr;
    FileTransferManager* fileTransfer_ = nullptr;
    tauricpp::Window* window_ = nullptr;  // Window reference for show/hide/close
    void* hwnd_ = nullptr;  // Main window handle for dialogs (cast to HWND in cpp)
    std::string dataDir_;   // Custom data directory (empty = use default)
    std::string minimizeBehavior_ = "taskbar";  // "taskbar" or "tray"
    bool notificationSound_ = true;  // play notification sound on new messages

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
    // The most recent FeiQ screenshot WE RECEIVED, kept verbatim so the UI can
    // "echo" it back to the sender byte-for-byte (diagnostic: proves whether our
    // send pipeline works even when we can't yet generate a valid image).
    std::string lastFeiQShotPayload_;   // full "LZW!" + size + crc + LZW(DIB)
    std::string lastFeiQShotSender_;    // sender key of that screenshot
    uint32_t lastFeiQShotRefCmd_ = 0;   // wire command FeiQ used for the ref msg
    uint32_t lastFeiQShotFragCmd_ = 0;  // wire command FeiQ used for fragments
    int lastFeiQShotW_ = 400;
    int lastFeiQShotH_ = 134;
    void HandleFeiQScreenshotReference(const ipmsg::MsgBuf& msg, const std::string& body);
    bool HandleFeiQScreenshotFragment(const ipmsg::MsgBuf& msg);
    void FinalizeFeiQScreenshot(const std::string& key);
};

/// Return the current user's Downloads folder, e.g. C:\Users\<user>\Downloads.
/// Falls back to USERPROFILE\Downloads if the known-folder lookup fails.
std::string GetUserDownloadsDir();

} // namespace ipmsg
