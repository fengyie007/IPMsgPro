// ============================================================================
// Bridge Command Handler Implementation
// ============================================================================

// Prevent windows.h from including winsock.h (which conflicts with winsock2.h)
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <shellapi.h>

#include "command_handler.h"
#include "ipmsg/protocol.h"
#include "logger.h"
#include "util/app_paths.h"
#include "util/encoding.h"
#include "util/base64.h"
#include "bridge/notification_sound.h"
#include "bridge/screen_capture.h"
#include <ctime>
#include <random>
#include <sstream>
#include <fstream>
#include <iomanip>
#include <cctype>
#include <cstring>
#include <cstdint>
#include <charconv>
#include <string_view>
#include <vector>
#include <tauricpp/dialog.hpp>
#include <shlobj.h>
#include <thread>
#include <iostream>
#include <chrono>
#include <filesystem>
#include <commdlg.h>

namespace fs = std::filesystem;

// ============================================================================
// Text encoding: single implementation in util/encoding.h. The short aliases
// keep the protocol code below readable (GBK = system ANSI code page).
// ============================================================================
static inline std::string GbkToUtf8(const std::string& s) { return ipmsg::enc::AnsiToUtf8(s); }
static inline std::string Utf8ToGbk(const std::string& s) { return ipmsg::enc::Utf8ToAnsi(s); }
static inline std::wstring Utf8ToWide(const std::string& s) { return ipmsg::enc::Utf8ToWide(s); }
static inline bool IsValidUtf8(const std::string& s) { return ipmsg::enc::IsValidUtf8(s); }
static inline std::string EnsureUtf8(const std::string& s) { return ipmsg::enc::EnsureUtf8(s); }

// Reduce a peer-supplied file name to a safe leaf name: drop any directory
// part (so "..\\x" or "C:\\y" cannot escape the target folder), replace the
// characters Windows forbids, strip trailing dots/spaces (Win32 drops them
// silently, which would alias another name) and avoid reserved device names.
// Never returns an empty string.
static std::string SanitizeFileName(const std::string& name) {
    size_t sep = name.find_last_of("/\\");
    std::string leaf = (sep == std::string::npos) ? name : name.substr(sep + 1);
    for (auto& c : leaf) {
        if (c == ':' || c == '*' || c == '?' || c == '"' || c == '<' || c == '>' || c == '|' ||
            static_cast<unsigned char>(c) < 0x20) {
            c = '_';
        }
    }
    while (!leaf.empty() && (leaf.back() == '.' || leaf.back() == ' ')) leaf.pop_back();
    if (leaf.empty()) return "file";

    std::string stem = leaf.substr(0, leaf.find('.'));
    std::transform(stem.begin(), stem.end(), stem.begin(), ::toupper);
    static const char* kReserved[] = {"CON", "PRN", "AUX", "NUL",
        "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"};
    for (const char* r : kReserved) {
        if (stem == r) return "_" + leaf;
    }
    return leaf;
}

// Return `dir\name`, or `dir\stem (n).ext` for the first n that does not exist
// yet, so an incoming file never silently overwrites an existing one.
static std::string UniqueSavePath(const std::string& dir, const std::string& name) {
    std::error_code ec;
    std::string candidate = dir + "\\" + name;
    if (!fs::exists(fs::path(Utf8ToWide(candidate)), ec)) return candidate;

    size_t dot = name.find_last_of('.');
    bool hasExt = (dot != std::string::npos && dot > 0);
    std::string stem = hasExt ? name.substr(0, dot) : name;
    std::string ext = hasExt ? name.substr(dot) : "";
    for (int n = 1; n < 10000; ++n) {
        candidate = dir + "\\" + stem + " (" + std::to_string(n) + ")" + ext;
        if (!fs::exists(fs::path(Utf8ToWide(candidate)), ec)) return candidate;
    }
    return candidate;
}

// FeiQ appends LOGFONT fields, a font face and COLORREF to plain text. Only
// strip a complete, well-formed suffix; ordinary braces and partial tags are text.
static std::string StripFeiQFontSuffix(const std::string& body) {
    constexpr std::string_view marker = "{/font;";
    const size_t start = body.rfind(marker);
    if (start == std::string::npos || body.size() - start > 512 ||
        body.size() - start < marker.size() + 2 || body.compare(body.size() - 2, 2, ";}") != 0) {
        return body;
    }
    std::string_view fields(body.data() + start + marker.size(),
                            body.size() - start - marker.size() - 2);
    if (fields.find_first_of("{};\r\n") != std::string_view::npos) return body;
    auto trimSpaces = [](std::string_view text) {
        const size_t first = text.find_first_not_of(" \t");
        if (first == std::string_view::npos) return std::string_view{};
        const size_t last = text.find_last_not_of(" \t");
        return text.substr(first, last - first + 1);
    };
    // Five LONG fields followed by eight BYTE fields (LOGFONT without lfFaceName).
    for (int i = 0; i < 13; ++i) {
        fields = trimSpaces(fields);
        const size_t end = fields.find_first_of(" \t");
        if (end == std::string_view::npos) return body;
        int32_t value = 0;
        const auto parsed = std::from_chars(fields.data(), fields.data() + end, value);
        if (parsed.ec != std::errc{} || parsed.ptr != fields.data() + end ||
            (i >= 5 && (value < 0 || value > 255))) return body;
        fields.remove_prefix(end + 1);
    }
    fields = trimSpaces(fields);
    // The final token is the color; the font face may itself contain spaces.
    const size_t colorStart = fields.find_last_of(" \t");
    if (colorStart == std::string_view::npos ||
        trimSpaces(fields.substr(0, colorStart)).empty()) return body;
    const std::string_view color = fields.substr(colorStart + 1);
    uint32_t value = 0;
    const auto parsed = std::from_chars(color.data(), color.data() + color.size(), value);
    if (parsed.ec != std::errc{} || parsed.ptr != color.data() + color.size()) return body;
    return body.substr(0, start);
}

// Toast text for a chat message body: emoji XML becomes "[表情]", line breaks become spaces.
static std::string NotificationPreview(const std::string& body) {
    static const std::string kEmojiOpen = "<msg><emoji ";
    static const std::string kEmojiClose = "</msg>";
    std::string text;
    size_t pos = 0;
    while (pos < body.size()) {
        const size_t start = body.find(kEmojiOpen, pos);
        const size_t end = (start == std::string::npos) ? std::string::npos : body.find(kEmojiClose, start);
        if (end == std::string::npos) {
            text.append(body, pos, std::string::npos);
            break;
        }
        text.append(body, pos, start - pos);
        text += "[表情]";
        pos = end + kEmojiClose.size();
    }
    std::string flat;
    for (char c : text) {
        if (c != '\r') flat += (c == '\n') ? ' ' : c;
    }
    return flat.find_first_not_of(' ') == std::string::npos ? "新消息" : flat;
}

namespace ipmsg {

CommandHandler& CommandHandler::Instance() {
    static CommandHandler instance;
    return instance;
}

void CommandHandler::Init(tauricpp::Bridge& bridge, MsgMng& msgMng,
                           MessageDB& msgDb, FileTransferManager& fileTransfer) {
    bridge_ = &bridge;
    msgMng_ = &msgMng;
    msgDb_ = &msgDb;
    fileTransfer_ = &fileTransfer;
    // Protocol diagnostics land in <dataDir>\debug when --verbose is active.
    feiqAssembler_.SetDebugDump([this](const std::string& name, const std::string& data) {
        DumpDebugFile(name, data);
    });
    feiqAssembler_.SetFragmentAck([this](const UserInfo& peer, const std::string& id, int index) {
        msgMng_->SendImagePacket(peer, IPMSG_REPORT_RECVIMAGE, id + "|" + std::to_string(index) + "#");
    });
    imageSender_.Init(msgMng, msgDb, [this](const FeiQImageSender::Task& task,
                                          const std::string& state, int progress, const std::string& error) {
        bridge_->Emit("image.send_" + state, {
            {"messageId", task.messageId}, {"imageId", task.imageId}, {"target", task.target.Key()},
            {"filePath", task.copied ? task.filePath : task.sourcePath},
            {"fileName", task.fileName}, {"fileSize", task.fileSize}, {"progress", progress}, {"error", error}
        });
    });
}

void CommandHandler::SetNativeWindowHandle(void* hwnd) {
    hwnd_ = static_cast<void*>(hwnd);
    LogMessage("BRIDGE", "DEBUG", "[DIALOG]SetNativeWindowHandle called, hwnd=" + 
                  (hwnd ? std::to_string(reinterpret_cast<uintptr_t>(hwnd)) : "NULL"));
}

void CommandHandler::SetWindow(tauricpp::Window* window) {
    window_ = window;
    LogMessage("BRIDGE", "", "[WINDOW] SetWindow called");
}

nlohmann::json CommandHandler::HandleWindowSetAlwaysOnTop(const nlohmann::json& args) {
    nlohmann::json r; r["success"] = true;
    try {
        bool onTop = args.contains("on_top") ? args["on_top"].get<bool>() : true;
        if (window_) window_->SetAlwaysOnTop(onTop);
    } catch (const std::exception& e) {
        r["success"] = false; r["error"] = e.what();
    }
    return r;
}

nlohmann::json CommandHandler::HandleWindowSetActiveConversation(const nlohmann::json& args) {
    std::lock_guard<std::mutex> lock(activeConversationMutex_);
    activeConversation_ = args.value("userId", "");
    return {{"success", true}};
}

// Toast + flashing when the window is not in front; the sound is skipped only
// while the user is reading this sender's conversation.
void CommandHandler::NotifyIncoming(const UserInfo& sender, const std::string& preview) {
    const bool inFront = window_ && window_->IsVisible() && !window_->IsMinimized() && window_->IsFocused();
    if (window_ && !inFront) {
        const std::string& name = sender.nickName.empty() ? sender.userName : sender.nickName;
        window_->ShowTrayNotification(EnsureUtf8(name), preview);
    }
    bool reading = false;
    if (inFront) {
        std::lock_guard<std::mutex> lock(activeConversationMutex_);
        reading = activeConversation_ == sender.Key();
    }
    if (notificationSound_ && !reading) {
        PlayNotificationSound();
    }
    LogMessage("BRIDGE", "DEBUG", "[NOTIFY] from=" + sender.Key() + " inFront=" + std::to_string(inFront) +
               " reading=" + std::to_string(reading) + " sound=" + std::to_string(notificationSound_ && !reading));
}

void CommandHandler::RegisterAllCommands() {
    if (!bridge_) return;

    // User management
    bridge_->RegisterCommand("user.discover",
        [this](const nlohmann::json& args) { return HandleUserDiscover(args); });
    bridge_->RegisterCommand("user.list",
        [this](const nlohmann::json& args) { return HandleUserList(args); });
    bridge_->RegisterCommand("user.status",
        [this](const nlohmann::json& args) { return HandleUserStatus(args); });
    bridge_->RegisterCommand("user.local",
        [this](const nlohmann::json& args) { return HandleUserLocal(args); });

    // Message
    bridge_->RegisterCommand("message.send",
        [this](const nlohmann::json& args) { return HandleMessageSend(args); });
    bridge_->RegisterCommand("image.send",
        [this](const nlohmann::json& args) { return HandleImageSend(args); });

    // File
    bridge_->RegisterCommand("file.send",
        [this](const nlohmann::json& args) { return HandleFileSend(args); });
    bridge_->RegisterCommand("file.info",
        [this](const nlohmann::json& args) { return HandleFileInfo(args); });
    bridge_->RegisterCommand("file.read_image",
        [this](const nlohmann::json& args) { return HandleFileReadImage(args); });
    bridge_->RegisterCommand("file.save_temp",
        [this](const nlohmann::json& args) { return HandleFileSaveTemp(args); });
    bridge_->RegisterCommand("file.accept",
        [this](const nlohmann::json& args) { return HandleFileAccept(args); });
    bridge_->RegisterCommand("file.reject",
        [this](const nlohmann::json& args) { return HandleFileReject(args); });
    bridge_->RegisterCommand("file.open_folder",
        [this](const nlohmann::json& args) { return HandleFileOpenFolder(args); });

    // History
    bridge_->RegisterCommand("history.get",
        [this](const nlohmann::json& args) { return HandleHistoryGet(args); });
    bridge_->RegisterCommand("history.search",
        [this](const nlohmann::json& args) { return HandleHistorySearch(args); });
    bridge_->RegisterCommand("history.clear",
        [this](const nlohmann::json& args) { return HandleHistoryClear(args); });
    bridge_->RegisterCommand("history.get_recent",
        [this](const nlohmann::json& args) { return HandleHistoryGetRecent(args); });

    // Network
    bridge_->RegisterCommand("network.scan",
        [this](const nlohmann::json& args) { return HandleNetworkScan(args); });
    bridge_->RegisterCommand("network.scan_range",
        [this](const nlohmann::json& args) { return HandleNetworkScanRange(args); });
    bridge_->RegisterCommand("network.scan_cancel",
        [this](const nlohmann::json& args) { return HandleNetworkScanCancel(args); });

    // Config
    bridge_->RegisterCommand("config.set",
        [this](const nlohmann::json& args) { return HandleConfigSet(args); });
    bridge_->RegisterCommand("config.loaded",
        [this](const nlohmann::json& args) { return HandleConfigLoaded(args); });
    bridge_->RegisterCommand("frontend.error",
        [this](const nlohmann::json& args) { return HandleFrontendError(args); });

    // Dialog
    bridge_->RegisterCommand("dialog.pick_folder",
        [this](const nlohmann::json& args) { return HandleDialogPickFolder(args); });
    bridge_->RegisterCommand("dialog.open",
        [this](const nlohmann::json& args) { return HandleDialogOpen(args); });
    bridge_->RegisterCommand("shell_open",
        [this](const nlohmann::json& args) { return HandleShellOpen(args); });

    // --- Screenshot commands ---
    bridge_->RegisterCommand("screenshot.capture",
        [this](const nlohmann::json& args) { return HandleScreenshotCapture(args); });
    bridge_->RegisterCommand("window.maximize",
        [this](const nlohmann::json& args) { return HandleWindowMaximize(args); });
    bridge_->RegisterCommand("window.restore",
        [this](const nlohmann::json& args) { return HandleWindowRestore(args); });
    bridge_->RegisterCommand("window.set_always_on_top",
        [this](const nlohmann::json& args) { return HandleWindowSetAlwaysOnTop(args); });
    bridge_->RegisterCommand("window.set_active_conversation",
        [this](const nlohmann::json& args) { return HandleWindowSetActiveConversation(args); });
    bridge_->RegisterCommand("dialog.save",
        [this](const nlohmann::json& args) { return HandleDialogSave(args); });
    bridge_->RegisterCommand("file.save_data",
        [this](const nlohmann::json& args) { return HandleFileSaveData(args); });

}

void CommandHandler::SetupEventForwarding() {
    if (!bridge_ || !msgMng_) return;

    // Setup file transfer progress callback
    fileTransfer_->SetProgressCallback([this](const ipmsg::TransferProgress& progress) {
        nlohmann::json event = {
            {"transferId", progress.transferId},
            {"filename", progress.filename},
            {"fileSize", progress.fileSize},
            {"transferred", progress.transferred},
            {"status", static_cast<int>(progress.status)},
            {"isSending", progress.isSending}
        };

        LogMessage("BRIDGE", "DEBUG", "[PROGRESS-CB] transferId=" + progress.transferId +
                     ", status=" + std::to_string(static_cast<int>(progress.status)) +
                     ", transferred=" + std::to_string(progress.transferred) +
                     "/" + std::to_string(progress.fileSize) +
                     ", isSending=" + std::to_string(progress.isSending));

        try {
            // The database record of a file/image message uses the transferId
            // as its id (both directions), so the final outcome can be persisted
            // here and survives a restart (see MessageStatus in message_db.h).
            if (progress.status == ipmsg::TransferStatus::Completed) {
                // File transfer completed
                event["message"] = progress.isSending ? "File sent successfully" : "File received successfully";
                if (!progress.isSending) {
                    event["savePath"] = progress.localPath;
                }
                if (msgDb_) msgDb_->UpdateStatus(progress.transferId, kMsgStatusCompleted);
                LogMessage("BRIDGE", "DEBUG", "[PROGRESS-CB] Emitting file.transfer_completed for transferId=" + progress.transferId);
                bridge_->Emit("file.transfer_completed", event);
            } else if (progress.status == ipmsg::TransferStatus::Failed) {
                // File transfer failed
                event["message"] = "File transfer failed";
                if (msgDb_) msgDb_->UpdateStatus(progress.transferId, kMsgStatusFailed);
                bridge_->Emit("file.transfer_failed", event);
            } else {
                // Progress update
                event["progress"] = progress.fileSize > 0 ?
                    (progress.transferred * 100.0 / progress.fileSize) : 0.0;
                bridge_->Emit("file.transfer_progress", event);
            }
        } catch (const std::exception& e) {
            LogMessage("BRIDGE", "", "[PROGRESS-CB] Emit threw for transferId=" + progress.transferId +
                          " filename=" + progress.filename + ": " + e.what());
        } catch (...) {
            LogMessage("BRIDGE", "", "[PROGRESS-CB] Emit threw unknown exception for transferId=" + progress.transferId);
        }
    });

    msgMng_->SetUserDiscoveredCallback([this](const ipmsg::UserInfo& user) {
        bridge_->Emit("user.discovered", UserToJson(user));
    });

    msgMng_->SetUserLeftCallback([this](const UserInfo& user) {
        bridge_->Emit("user.status_changed", {
            {"user", UserToJson(user)},
            {"status", "offline"}
        });
    });

    msgMng_->SetMessageReceivedCallback([this](const MsgBuf& msg) {
        try {
            // Log every received message (DEBUG only: ~12 lines per packet)
            if (IsDebugEnabled()) {
                char cmdBuf[32] = {};
                snprintf(cmdBuf, sizeof(cmdBuf), "0x%08lx", (unsigned long)msg.command);
                uint32_t mode = GET_MODE(msg.command);
                std::string modeStr;
                switch (mode) {
                    case IPMSG_BR_ENTRY: modeStr = "BR_ENTRY"; break;
                    case IPMSG_BR_EXIT: modeStr = "BR_EXIT"; break;
                    case IPMSG_ANSENTRY: modeStr = "ANSENTRY"; break;
                    case IPMSG_SENDMSG: modeStr = "SENDMSG"; break;
                    case IPMSG_RECVMSG: modeStr = "RECVMSG"; break;
                    default: modeStr = "UNKNOWN"; break;
                }
                LogMessage("BRIDGE", "DEBUG", "[GUI-MSG] ====== BEGIN MESSAGE ======");
                LogMessage("BRIDGE", "DEBUG", "[GUI-MSG] packetNo=" + std::to_string(msg.packetNo));
                LogMessage("BRIDGE", "DEBUG", std::string("[GUI-MSG] from=") + msg.sender.userName + "@" +
                              msg.sender.hostName + " (" + msg.sender.ipAddress + ":" +
                              std::to_string(msg.sender.portNo) + ")");
                LogMessage("BRIDGE", "DEBUG", std::string("[GUI-MSG] nickName=") + msg.sender.nickName +
                              ", groupName=" + msg.sender.groupName);
                LogMessage("BRIDGE", "DEBUG", std::string("[GUI-MSG] command=") + cmdBuf + " (" + modeStr + ")");
                {
                    std::string flags;
                    if (msg.command & IPMSG_SENDCHECKOPT) flags += "SENDCHECKOPT ";
                    if (msg.command & IPMSG_FILEATTACHOPT) flags += "FILEATTACHOPT ";
                    if (msg.command & IPMSG_UTF8OPT) flags += "UTF8OPT ";
                    if (msg.command & IPMSG_CAPUTF8OPT) flags += "CAPUTF8OPT ";
                    LogMessage("BRIDGE", "DEBUG", "[GUI-MSG] command_flags: " + flags);
                }
                LogMessage("BRIDGE", "DEBUG", std::string("[GUI-MSG] body=\"") + msg.body + "\" (len=" + std::to_string(msg.body.size()) + ")");
                LogMessage("BRIDGE", "DEBUG", std::string("[GUI-MSG] extra=\"") + msg.extra + "\" (len=" + std::to_string(msg.extra.size()) + ")");
                {
                    std::ostringstream hexOs;
                    hexOs << std::hex << std::setfill('0') << std::setw(2);
                    for (size_t i = 0; i < msg.extra.size() && i < 200; ++i) {
                        hexOs << (unsigned int)(unsigned char)msg.extra[i] << " ";
                    }
                    LogMessage("BRIDGE", "DEBUG", "[GUI-MSG] extra_hex: " + hexOs.str());
                }
                LogMessage("BRIDGE", "DEBUG", "[GUI-MSG] ====== END MESSAGE ======");
                LogMessage("BRIDGE", "DEBUG", std::string("[GUI-MSG] from=") + msg.sender.userName + "@" +
                              msg.sender.ipAddress + ":" + std::to_string(msg.sender.portNo) +
                              " cmd=" + cmdBuf + " mode=" + modeStr +
                              " body=\"" + msg.body + "\" extra=\"" + msg.extra + "\"");
            }

            // RECVMSG is the delivery receipt. Its BODY carries the packetNo of
            // OUR message being acknowledged (the header packetNo is the
            // receipt's own number). Resolve it to the database id handed to the
            // frontend by message.send, persist "delivered", then forward.
            uint32_t mode = GET_MODE(msg.command);
            if (mode == IPMSG_REPORT_RECVIMAGE || mode == IPMSG_RECVMSG) imageSender_.HandleAck(msg);
            if (mode == IPMSG_REPORT_RECVIMAGE) return;
            if (mode == IPMSG_RECVMSG) {
                uint64_t ackedPacketNo = 0;
                try { ackedPacketNo = std::stoull(msg.body); } catch (...) {}
                std::string messageId = ackedPacketNo ? TakePendingAck(ackedPacketNo) : std::string();
                if (!messageId.empty() && msgDb_) {
                    msgDb_->UpdateStatus(messageId, kMsgStatusDelivered);
                }
                bridge_->Emit("message.ack", {
                    {"packetNo", ackedPacketNo},
                    {"from", msg.sender.Key()},
                    {"messageId", messageId}
                });
                return;
            }

            // Ignore our own messages looping back. A peer is "us" only when key
            // AND port match (same rule as MsgMng::ProcessRecvBuffer): a second
            // instance on this machine shares user@host but listens elsewhere.
            const UserInfo local = msgMng_->GetLocalUser();
            const bool isSelfEcho = msg.sender.Key() == local.Key() && msg.sender.portNo == local.portNo;
            if (isSelfEcho) {
                return;
            }

            // --- FeiQ inline screenshot (custom fragmented image protocol) ---
            // Reassembly happens in FeiQScreenshotAssembler; the finished image
            // is forwarded to the frontend as an inline data URL.
            if (FeiQScreenshotAssembler::IsReference(msg.body)) {
                feiqAssembler_.HandleReference(msg);
                // Acknowledge read receipt if FeiQ requested one (SENDCHECKOPT)
                if (msg.command & IPMSG_SENDCHECKOPT) {
                    ipmsg::UserInfo u = msg.sender;
                    msgMng_->SendRecvMsg(u, msg.packetNo);
                }
                return;
            }
            if (FeiQScreenshotAssembler::IsFragment(msg.command, msg.body)) {
                std::optional<FeiQScreenshotResult> shot;
                if (feiqAssembler_.HandleFragment(msg, shot)) {
                    if (shot) EmitFeiQScreenshot(*shot);
                    return;
                }
            }

            // Only handle SENDMSG from here onwards
            if (mode != IPMSG_SENDMSG) return;

            // Ensure all sender fields are UTF-8 (FeiQ may send GBK even without UTF8OPT flag)
            auto& sender = const_cast<UserInfo&>(msg.sender);
            if (!(msg.command & IPMSG_UTF8OPT)) {
                // EnsureUtf8 is idempotent - safe to call even if already UTF-8
                sender.nickName = EnsureUtf8(sender.nickName);
                sender.groupName = EnsureUtf8(sender.groupName);
            }

            // Ensure extra is UTF-8 before constructing JSON
            std::string extraUtf8 = EnsureUtf8(msg.extra);

            // Determine message type based on command flags
            bool isFileAttach = (msg.command & IPMSG_FILEATTACHOPT) != 0;
            const std::string content = isFileAttach ? msg.body : StripFeiQFontSuffix(msg.body);

            std::string msgType = "text";
            int dbType = 0;  // 0:text, 1:image, 2:file
            std::string fileName;
            int64_t fileSize = 0;
            int fileId = 0;

            if (isFileAttach && !extraUtf8.empty()) {
                // Parse file attachment info in IPMsg/Feiq format:
                // "fileId:filename:hexSize:hexMtime:hexFileType:\a"
                // Multiple files are separated by \a (0x07)
                // Colons in filenames are escaped as ::
                // Reference: Feiq feiqengine.cpp RecvFile::createFileContent

                // Parse fields from extra, handling :: escape and \a separator
                auto parseFileAttachInfo = [](const std::string& extra,
                                             int& outFileId, std::string& outFileName,
                                             int64_t& outFileSize) {
                    // Split by \a (0x07) for multiple files - take first file only
                    std::string fileInfo = extra;
                    auto sepPos = extra.find('\x07');
                    if (sepPos != std::string::npos) {
                        fileInfo = extra.substr(0, sepPos);
                    }

                    // Parse colon-separated fields, with :: escape
                    std::vector<std::string> fields;
                    std::string current;
                    for (size_t i = 0; i < fileInfo.size(); i++) {
                        if (fileInfo[i] == ':') {
                            if (i + 1 < fileInfo.size() && fileInfo[i + 1] == ':') {
                                // Escaped colon ::
                                current += ':';
                                i++; // skip next colon
                            } else {
                                // Field separator
                                fields.push_back(current);
                                current.clear();
                            }
                        } else {
                            current += fileInfo[i];
                        }
                    }
                    fields.push_back(current); // last field

                    // Need at least 3 fields: fileId, filename, size
                    if (fields.size() < 3) return;

                    // IPMsg format: "fileNo:filename:hexSize:hexMtime:hexFileAttr[:extend-attr]"
                    // Field 0: fileNo (decimal) - used as fileId in GETFILEDATA
                    try { outFileId = std::stoi(fields[0]); } catch (...) {}

                    // Field 1: filename
                    outFileName = fields[1];

                    // Field 2: fileSize (hexadecimal)
                    try { outFileSize = std::stoll(fields[2], nullptr, 16); } catch (...) {}

                    // Field 3: mtime (hex, NOT used for GETFILEDATA)
                    // Field 4: fileAttr (hex, e.g. IPMSG_FILE_REGULAR=1)
                };

                parseFileAttachInfo(msg.extra, fileId, fileName, fileSize);

                // Log the raw extra bytes for comparison with our sending format
                {
                    std::ostringstream extraDbg;
                    extraDbg << "[RECV-FILE-EXTRA] Raw extra (" << msg.extra.size() << " bytes): ";
                    for (size_t i = 0; i < msg.extra.size() && i < 200; i++) {
                        unsigned char c = static_cast<unsigned char>(msg.extra[i]);
                        if (c == '\0') extraDbg << "\\0";
                        else if (c == '\x07') extraDbg << "\\a";
                        else if (c == '\n') extraDbg << "\\n";
                        else if (c >= 32 && c < 127) extraDbg << c;
                        else extraDbg << "<" << std::hex << (int)c << ">";
                    }
                    LogMessage("BRIDGE", "DEBUG", extraDbg.str());
                }
                {
                    std::ostringstream detailOs;
                    detailOs << std::dec << "[RECV-FILE-EXTRA] Parsed: fileId=" << fileId 
                             << ", fileName=" << fileName << ", fileSize=" << fileSize;
                    LogMessage("BRIDGE", "DEBUG", detailOs.str());
                }
                {
                    std::ostringstream detailOs;
                    detailOs << "[RECV-FILE-EXTRA] Original msg: packetNo=" << msg.packetNo 
                             << ", cmd=0x" << std::hex << msg.command << std::dec
                             << ", body='" << msg.body << "'";
                    LogMessage("BRIDGE", "DEBUG", detailOs.str());
                }

                // Determine if image or file based on extension
                std::string ext = fileName;
                auto dotPos = ext.find_last_of('.');
                if (dotPos != std::string::npos) {
                    ext = ext.substr(dotPos + 1);
                } else {
                    ext.clear();
                }
                std::transform(ext.begin(), ext.end(), ext.begin(), ::tolower);

                if (ext == "png" || ext == "jpg" || ext == "jpeg" || ext == "gif" ||
                    ext == "bmp" || ext == "webp") {
                    msgType = "image";
                    dbType = 1;
                } else {
                    msgType = "file";
                    dbType = 2;
                }
            }

            // Message id = sender key + packetNo. packetNo alone is only unique
            // per sender (each peer counts from its own start time), so two
            // peers could produce the same id and INSERT OR IGNORE would then
            // silently drop the second message from the history.
            const std::string messageId = msg.sender.Key() + ":" + std::to_string(msg.packetNo);
            nlohmann::json j = {
                {"id", messageId},
                {"from", msg.sender.Key()},
                {"fromUser", UserToJson(msg.sender)},
                {"content", content},
                {"type", msgType},
                {"timestamp", static_cast<int64_t>(msg.timestamp)},
                {"command", msg.command},
                {"extra", extraUtf8}
            };

            // Save to database (skip file attachments - they're managed by frontend via file.receive_request)
            // File attachment messages will be saved when the transfer completes
            if (!isFileAttach) {
                MessageRecord record;
                record.id = messageId;
                record.fromId = msg.sender.Key();
                record.toId = msgMng_->GetLocalUser().Key();
                record.content = content;
                record.type = dbType;
                record.timestamp = static_cast<int64_t>(msg.timestamp);
                record.status = kMsgStatusDelivered;
                msgDb_->SaveMessage(record);
            }

            // Emit message received event
            bridge_->Emit("message.received", j);

            // File attachments notify below, once their receive request is out.
            if (!isFileAttach) {
                NotifyIncoming(msg.sender, NotificationPreview(content));
            }

            // Do NOT auto-reply RECVMSG for file attachment notifications
            // RECVMSG should be sent when user clicks "Accept", not when notification is received
            // FeiQ interprets RECVMSG as "user accepted the file transfer"
            // If (isFileAttach) { ... }

            // If file attachment, emit file receive request event (NOT auto-accepting)
            if (isFileAttach && !fileName.empty()) {
                LogMessage("BRIDGE", "DEBUG", "[FILE_REQ_EMIT]packetNo=" + std::to_string(msg.packetNo) +
                              ", fromUser=" + msg.sender.Key() +
                              ", fromIp=" + msg.sender.ipAddress +
                              ", fromPort=" + std::to_string(msg.sender.portNo) +
                              ", fileName=" + fileName + 
                              ", fileSize=" + std::to_string(fileSize) +
                              ", fileId=" + std::to_string(fileId) +
                              ", transferId=" + std::to_string(msg.packetNo));
                
                LogMessage("BRIDGE", "DEBUG", "[BRIDGE_EMIT]Emitting file.receive_request event");
                bridge_->Emit("file.receive_request", {
                    {"packetNo", msg.packetNo},
                    {"fromUser", msg.sender.Key()},
                    {"fromUserIp", msg.sender.ipAddress},
                    {"fromUserPort", msg.sender.portNo},
                    // 接收方文件名来自协议（GBK），转 UTF-8 供前端正确显示
                    {"fileName", EnsureUtf8(fileName)},
                    {"fileSize", fileSize},
                    {"fileId", fileId},
                    {"transferId", std::to_string(msg.packetNo)}
                });

                NotifyIncoming(msg.sender, dbType == 1 ? std::string("[图片]") : "[文件] " + EnsureUtf8(fileName));
            }
        } catch (const std::exception& e) {
            LogMessage("BRIDGE", "DEBUG", std::string("[GUI-MSG] Exception in message callback: ") + e.what());
        } catch (...) {
            LogMessage("BRIDGE", "DEBUG", "[GUI-MSG] Unknown exception in message callback");
        }
    });

    msgMng_->SetUserStatusChangedCallback([this](const UserInfo& user) {
        bool isAway = (user.hostStatus & IPMSG_ABSENCEOPT) != 0;
        bridge_->Emit("user.status_changed", {
            {"user", UserToJson(user)},
            {"status", isAway ? "away" : "online"}
        });
    });

    // IP range scan progress. Registered once here (not per scan request) so
    // the scan thread never reads a callback while the UI thread replaces it,
    // and scans started from config.loaded also report to the frontend.
    msgMng_->SetScanProgressCallback([this](uint32_t current, uint32_t total, uint32_t found) {
        bridge_->Emit("network.scan_progress", {
            {"current", current},
            {"total", total},
            {"found", found}
        });
    });
    msgMng_->SetScanCompleteCallback([this](uint32_t found) {
        bridge_->Emit("network.scan_complete", {
            {"found", found}
        });
    });
}

// ---------- User Commands ----------

nlohmann::json CommandHandler::HandleUserDiscover(const nlohmann::json& args) {
    msgMng_->BroadcastEntry();
    return {{"success", true}};
}

nlohmann::json CommandHandler::HandleUserList(const nlohmann::json& args) {
    auto users = msgMng_->GetUsers();
    nlohmann::json userList = nlohmann::json::array();
    for (const auto& u : users) {
        userList.push_back(UserToJson(u));
    }
    return {{"users", userList}, {"count", users.size()}};
}

nlohmann::json CommandHandler::HandleUserStatus(const nlohmann::json& args) {
    std::string status = args.value("status", "online");
    uint32_t cmd = (status == "away") ?
        (IPMSG_BR_ABSENCE | IPMSG_ABSENCEOPT) :
        IPMSG_BR_ABSENCE;
    msgMng_->BroadcastAbsence(cmd);
    return {{"success", true}, {"status", status}};
}

nlohmann::json CommandHandler::HandleUserLocal(const nlohmann::json& args) {
    const UserInfo localUser = msgMng_->GetLocalUser();
    return {
        {"success", true},
        {"id", localUser.Key()},
        {"nickname", localUser.nickName},
        {"username", localUser.userName},
        {"hostname", localUser.hostName},
        {"group", localUser.groupName},
        {"ip", localUser.ipAddress},
        {"port", localUser.portNo}
    };
}

nlohmann::json CommandHandler::HandleConfigSet(const nlohmann::json& args) {
    if (args.contains("minimizeBehavior") &&
        (!args["minimizeBehavior"].is_string() ||
         (args["minimizeBehavior"] != "tray" && args["minimizeBehavior"] != "taskbar"))) {
        return {{"success", false}, {"error", "Invalid minimizeBehavior"}};
    }
    std::string dataDir = args.value("dataDir", "");

    // The frontend sends nickname and group in separate config.set calls, so
    // only the fields present in THIS call may be applied. Passing both
    // unconditionally let whichever call came last wipe the other field.
    std::optional<std::string> nickname;
    std::optional<std::string> group;
    if (args.contains("nickname") && args["nickname"].is_string()) {
        nickname = args["nickname"].get<std::string>();
    }
    if (args.contains("group") && args["group"].is_string()) {
        group = args["group"].get<std::string>();
    }
    if (nickname || group) {
        msgMng_->UpdateLocalInfo(nickname, group);
        LogMessage("BRIDGE", "", "Config updated: nickname=" + nickname.value_or("(unchanged)") +
                   ", group=" + group.value_or("(unchanged)"));
    }

    // Store custom data directory (used for downloads, database, etc.)
    if (!dataDir.empty()) {
        dataDir_ = dataDir;
        // Paths are UTF-8; the *A APIs would create a mojibake directory for
        // non-ASCII names, and the registry copy read back at the next start
        // would not match what SQLite (which takes UTF-8) opens.
        const std::string effectiveDir = GetDataDir();
        {
            std::error_code ec;
            fs::create_directories(enc::PathFromUtf8(effectiveDir), ec);
        }
        LogMessage("BRIDGE", "", "Config updated: dataDir=" + dataDir_ + " (effective: " + effectiveDir + ")");

        // Save to registry so it's available at next startup before frontend loads
        paths::WriteCustomDataDir(dataDir_);

        // Reinitialize logger to new data directory
        ipmsg::ReinitLogger(effectiveDir);

        // Re-initialize database with new data directory. Init() swaps the
        // connection under the database mutex; an explicit Close() first would
        // open a window where the receive thread drops incoming messages.
        if (msgDb_) {
            std::string dbPath = effectiveDir + "\\ipmsg.db";
            if (!msgDb_->Init(dbPath)) {
                LogMessage("BRIDGE", "ERROR", "Failed to reinitialize database at " + dbPath);
            } else {
                LogMessage("BRIDGE", "", "[BRIDGE] Database reinitialized at " + dbPath);
            }
        }
    } else if (args.contains("dataDir") && args["dataDir"].is_string() && args["dataDir"].get<std::string>().empty()) {
        // dataDir explicitly set to empty -> reset to default
        dataDir_.clear();
        LogMessage("BRIDGE", "", "Config updated: dataDir reset to default");

        // Remove from registry
        paths::WriteCustomDataDir("");

        // Reinitialize logger to default data directory
        std::string defaultDir = GetDataDir();
        ipmsg::ReinitLogger(defaultDir);

        // Re-initialize database with default data directory (see above)
        if (msgDb_) {
            std::string dbPath = defaultDir + "\\ipmsg.db";
            if (!msgDb_->Init(dbPath)) {
                LogMessage("BRIDGE", "ERROR", "Failed to reinitialize database at " + dbPath);
            } else {
                LogMessage("BRIDGE", "", "[BRIDGE] Database reinitialized at " + dbPath);
            }
        }
    }

    // Store minimize behavior setting
    if (args.contains("minimizeBehavior")) {
        minimizeBehavior_ = args.value("minimizeBehavior", "taskbar");
        LogMessage("BRIDGE", "", "Config updated: minimizeBehavior=" + minimizeBehavior_);
    }

    // Store notification sound setting
    if (args.contains("notificationSound")) {
        notificationSound_ = args.value("notificationSound", true);
        LogMessage("BRIDGE", "", "Config updated: notificationSound=" + std::string(notificationSound_ ? "true" : "false"));
    }

    // Sync custom broadcast segments from frontend config
    if (args.contains("segments") && args["segments"].is_array()) {
        // Clear existing custom segments (auto-detected ones are rebuilt by GetAllBroadcastAddresses)
        auto current = msgMng_->GetSegments();
        for (const auto& seg : current) {
            msgMng_->RemoveSegment(seg);
        }
        // Add new segments from config
        for (const auto& seg : args["segments"]) {
            if (seg.is_string()) {
                msgMng_->AddSegment(seg.get<std::string>());
            }
        }
        LogMessage("BRIDGE", "", "Config updated: segments synced (" +
                   std::to_string(args["segments"].size()) + " entries)");
    }

    // Sync direct users (cross-subnet) — clear first to match config exactly
    if (args.contains("directUsers") && args["directUsers"].is_array()) {
        // Clear existing direct users
        msgMng_->ClearDirectUsers();
        // Add new direct users from config and send BR_ENTRY immediately
        for (const auto& user : args["directUsers"]) {
            if (user.is_string()) {
                std::string entry = user.get<std::string>();
                size_t colonPos = entry.find(':');
                if (colonPos == std::string::npos) continue;
                std::string ip = entry.substr(0, colonPos);
                // A malformed port ("10.8.33.50:abc") must skip this entry, not
                // throw out of the whole config.set call.
                int port = 0;
                try { port = std::stoi(entry.substr(colonPos + 1)); } catch (...) { port = 0; }
                if (ip.empty() || port <= 0 || port > 65535) {
                    LogMessage("BRIDGE", "WARN", "Config: ignoring invalid direct user entry \"" + entry + "\"");
                    continue;
                }
                msgMng_->AddDirectUser(ip, port);
                // Send BR_ENTRY immediately so user appears without restart
                msgMng_->SendDirectEntry(ip, port);
            }
        }
        LogMessage("BRIDGE", "", "Config updated: directUsers synced (" +
                   std::to_string(args["directUsers"].size()) + " entries)");
    }

    // Sync IP scan ranges — clear first to match config exactly
    if (args.contains("ipScanRanges") && args["ipScanRanges"].is_array()) {
        // Clear existing scan ranges
        msgMng_->ClearScanRanges();
        // Add new scan ranges from config
        for (const auto& range : args["ipScanRanges"]) {
            if (range.is_string()) {
                std::string rangeStr = range.get<std::string>();
                msgMng_->AddScanRange(rangeStr);
            }
        }
        LogMessage("BRIDGE", "", "Config updated: ipScanRanges synced (" +
                   std::to_string(args["ipScanRanges"].size()) + " entries)");
    }

    return {{"success", true}};
}

// Called after frontend finishes loading config from IndexedDB
nlohmann::json CommandHandler::HandleConfigLoaded(const nlohmann::json& args) {
    // Send BR_ENTRY to all configured direct users (cross-subnet)
    for (const auto& [ip, port] : msgMng_->GetDirectUsers()) {
        LogMessage("BRIDGE", "", "Config loaded: auto-adding direct user " + ip + ":" + std::to_string(port));
        msgMng_->SendDirectEntry(ip, port);
    }

    // Auto-scan IP ranges from config. This is the single startup trigger; the
    // frontend must not start another scan after config.loaded. Peers are
    // expected on the same port we listen on (not always the default 2425).
    auto scanRanges = msgMng_->GetScanRanges();
    if (!scanRanges.empty()) {
        LogMessage("BRIDGE", "", "Config loaded: auto-scanning " + std::to_string(scanRanges.size()) + " IP ranges");
        msgMng_->ScanIpRanges(scanRanges, msgMng_->GetLocalPort(), 50);
    }
    return {{"success", true}};
}

nlohmann::json CommandHandler::HandleFrontendError(const nlohmann::json& args) {
    std::string message = args.value("message", "Unknown error");
    std::string stack = args.value("stack", "");
    LogMessage("BRIDGE", "ERROR", "[FRONTEND ERROR] " + message + (stack.empty() ? "" : "\nStack: " + stack));
    return {{"success", true}};
}

// ---------- Message Commands ----------

nlohmann::json CommandHandler::HandleMessageSend(const nlohmann::json& args) {
    auto target = FindUserFromArgs(args);
    if (!target) {
        return {{"success", false}, {"error", "Target user not found"}};
    }

    std::string content = args.value("content", "");
    if (content.empty()) {
        return {{"success", false}, {"error", "Message content is empty"}};
    }

    LogMessage("BRIDGE", "DEBUG", "[BACKEND-SEND]TEXT to=" + target->Key() + ", content=\"" + content + "\"");

    // Normal mode: send via UDP
    // Try UTF-8 first (with IPMSG_UTF8OPT flag), fallback to GBK if needed.
    // Feiq/FeiQ handles IPMSG_UTF8OPT properly for UTF-8 encoded content.
    // For other IPMsg clients that don't understand UTF8OPT (like older FeiQ),
    // we send GBK encoded content without the UTF8 flag.
    uint64_t sentPacketNo = msgMng_->SendMessage(*target, content, IPMSG_SENDCHECKOPT);
    bool ok = sentPacketNo != 0;

    // Always save to database (even if send failed, we want to track it)
    MessageRecord record;
    {
        // Generate unique ID: timestamp_ms + random suffix to avoid collision
        auto now = std::chrono::system_clock::now().time_since_epoch().count();
        std::random_device rd;
        std::mt19937 gen(rd());
        std::uniform_int_distribution<> dis(1000, 9999);
        record.id = std::to_string(now) + "_" + std::to_string(dis(gen));
    }
    record.fromId = msgMng_->GetLocalUser().Key();
    record.toId = target->Key();
    record.content = content;
    record.type = 0;  // text
    record.timestamp = static_cast<int64_t>(std::time(nullptr));
    record.status = ok ? kMsgStatusSending : kMsgStatusFailed;
    msgDb_->SaveMessage(record);

    // Let the peer's RECVMSG receipt be resolved back to this record.
    if (ok) {
        RegisterPendingAck(sentPacketNo, record.id);
    }

    return {{"success", ok}, {"messageId", record.id}};
}

nlohmann::json CommandHandler::HandleImageSend(const nlohmann::json& args) {
    auto target = FindUserFromArgs(args);
    if (!target) return {{"success", false}, {"error", "未找到目标用户"}};
    const std::string source = args.value("filePath", "");
    if (source.empty()) return {{"success", false}, {"error", "请选择图片文件"}};
    FeiQImageSender::Task task;
    std::string error;
    if (!imageSender_.Enqueue(*target, source, GetDataDir(), task, error))
        return {{"success", false}, {"error", error}};
    return {{"success", true}, {"messageId", task.messageId}, {"imageId", task.imageId},
            {"filePath", task.filePath}, {"fileName", task.fileName}, {"fileSize", task.fileSize}};
}

// ---------- File Commands ----------

nlohmann::json CommandHandler::HandleFileSend(const nlohmann::json& args) {
    auto target = FindUserFromArgs(args);
    if (!target) {
        return {{"success", false}, {"error", "Target user not found"}};
    }

    std::string filePath = args.value("filePath", "");
    if (filePath.empty()) {
        return {{"success", false}, {"error", "File path is empty"}};
    }

    LogMessage("BRIDGE", "DEBUG", "[BACKEND-SEND]FILE to=" + target->Key() + ", filePath=\"" + filePath + "\"");

    // Start TCP file transfer (register file info for serving)
    std::string transferId = fileTransfer_->StartSendFile(
        target->ipAddress, target->portNo, filePath, target->Key());

    if (transferId.empty()) {
        return {{"success", false}, {"error", "Failed to start file transfer"}};
    }

    // Get file info
    auto fileInfo = fileTransfer_->GetFileInfo(transferId);
    if (!fileInfo) {
        return {{"success", false}, {"error", "Failed to get file info"}};
    }

    // Build file attach info for IPMsg protocol (Feiq format):
    // "fileId:filename:hexSize:hexMtime:hexFileType:\a"
    std::ostringstream attachOs;
    // 协议层文件名必须是 GBK（飞秋/原生 UI 按 ANSI 解析），内部 fileInfo->fileName 是 UTF-8
    std::string escapedFileName = Utf8ToGbk(fileInfo->fileName);
    // Escape colons in filename (:: represents a literal colon)
    {
        std::string escaped;
        for (char c : escapedFileName) {
            if (c == ':') escaped += "::";
            else escaped += c;
        }
        escapedFileName = escaped;
    }
    attachOs << fileInfo->fileId << ":" << escapedFileName << ":"
             << std::hex << fileInfo->fileSize << ":"
             << fileInfo->modifyTime << ":"
             << fileInfo->fileAttr << ":\x07";
    std::string fileAttachInfo = attachOs.str();

    // Debug: print the file attach info with visible control chars
    {
        std::ostringstream dbgOs;
        for (size_t i = 0; i < fileAttachInfo.size(); i++) {
            unsigned char c = static_cast<unsigned char>(fileAttachInfo[i]);
            if (c == '\0') dbgOs << "\\0";
            else if (c == '\x07') dbgOs << "\\a";
            else if (c == '\n') dbgOs << "\\n";
            else if (c >= 32 && c < 127) dbgOs << c;
            else dbgOs << "<" << std::hex << (int)c << ">";
        }
        LogMessage("BRIDGE", "DEBUG", "[SEND-FILE-EXTRA] " + dbgOs.str());
        std::ostringstream detailOs;
        detailOs << std::dec << "[SEND-FILE-EXTRA] fileId=" << fileInfo->fileId 
                  << ", fileName=" << fileInfo->fileName
                  << ", fileSize=" << fileInfo->fileSize
                  << ", modifyTime=" << fileInfo->modifyTime
                  << ", fileAttr=" << fileInfo->fileAttr;
        LogMessage("BRIDGE", "DEBUG", detailOs.str());
    }

    // Normal mode: send UDP notification with file attachment info
    LogMessage("BRIDGE", "DEBUG", "[BACKEND-SEND]Sending SENDMSG with FILEATTACHOPT to " + target->Key() +
                  " (fileId=" + std::to_string(fileInfo->fileId) + ", fileSize=" + std::to_string(fileInfo->fileSize) + ")");
    LogMessage("BRIDGE", "DEBUG", "[BACKEND-SEND]File attach info: " + fileAttachInfo);
    
    // 通知消息文本里的文件名也用 GBK，避免飞秋消息列表里乱码
    uint64_t sentPktNo = msgMng_->SendMessageWithFile(*target, "[File: " + Utf8ToGbk(fileInfo->fileName) + "]", fileAttachInfo, IPMSG_SENDCHECKOPT);

    if (sentPktNo > 0) {
        LogMessage("BRIDGE", "DEBUG", "[BACKEND-SEND]SENDMSG sent successfully, packetNo=" + std::to_string(sentPktNo));
        // Store the SENDMSG packetNo in FileInfo for matching GETFILEDATA requests
        {
            auto fi = fileTransfer_->GetFileInfo(transferId);
            if (fi) {
                fi->packetNo = sentPktNo;
                fileTransfer_->RegisterFileInfo(transferId, *fi);
                LogMessage("BRIDGE", "DEBUG", "[BACKEND-SEND]FileInfo updated: packetNo=" + std::to_string(sentPktNo) + ", fileId=" + std::to_string(fi->fileId));
            }
        }
        // Save to database
        MessageRecord record;
        record.id = transferId;
        record.fromId = msgMng_->GetLocalUser().Key();
        record.toId = target->Key();
        record.content = filePath;  // Store file path
        record.type = 2;  // file
        record.timestamp = static_cast<int64_t>(std::time(nullptr));
        record.status = kMsgStatusSending;  // completed/failed is written by the progress callback
        msgDb_->SaveMessage(record);

        // Emit transfer started event
        bridge_->Emit("file.transfer_started", {
            {"transferId", transferId},
            {"filename", fileInfo->fileName},
            {"fileSize", fileInfo->fileSize},
            {"isSending", true},
            {"targetUser", target->Key()}
        });
    }

    return {{"success", true}, {"transferId", transferId}, {"fileName", fileInfo->fileName}};
}

nlohmann::json CommandHandler::HandleFileInfo(const nlohmann::json& args) {
    std::string filePath = args.value("filePath", "");
    if (filePath.empty()) {
        return {{"success", false}, {"error", "File path is empty"}};
    }
    try {
        std::error_code ec;
        uint64_t size = fs::file_size(fs::u8path(filePath), ec);
        if (ec) {
            return {{"success", false}, {"error", ec.message()}};
        }
        std::string name = fs::u8path(filePath).filename().u8string();
        return {{"success", true}, {"fileSize", size}, {"fileName", name}};
    } catch (const std::exception& e) {
        return {{"success", false}, {"error", std::string(e.what())}};
    }
}

// Read a local image file and return it as a data: URL so the chat bubble can
// show a thumbnail. Images sent/received through the standard file channel
// only exist as paths on disk; the WebView cannot read them directly.
nlohmann::json CommandHandler::HandleFileReadImage(const nlohmann::json& args) {
    std::string filePath = args.value("filePath", "");
    if (filePath.empty()) {
        return {{"success", false}, {"error", "File path is empty"}};
    }

    // Only image extensions, and a size cap so a huge photo cannot balloon the
    // frontend's memory (the bubble is a thumbnail; click opens the original).
    static const std::map<std::string, std::string> kMime = {
        {"png", "image/png"}, {"jpg", "image/jpeg"}, {"jpeg", "image/jpeg"},
        {"gif", "image/gif"}, {"bmp", "image/bmp"}, {"webp", "image/webp"},
    };
    constexpr uint64_t kMaxBytes = 5 * 1024 * 1024;

    std::string ext;
    if (auto dot = filePath.find_last_of('.'); dot != std::string::npos) {
        ext = filePath.substr(dot + 1);
        std::transform(ext.begin(), ext.end(), ext.begin(), ::tolower);
    }
    auto mimeIt = kMime.find(ext);
    if (mimeIt == kMime.end()) {
        return {{"success", false}, {"error", "Not an image file"}};
    }

    std::error_code ec;
    const fs::path p = enc::PathFromUtf8(filePath);
    uint64_t size = fs::file_size(p, ec);
    if (ec) {
        return {{"success", false}, {"error", ec.message()}};
    }
    if (size > kMaxBytes) {
        return {{"success", false}, {"error", "Image too large for preview"}, {"tooLarge", true}};
    }

    std::ifstream in(p, std::ios::binary);
    if (!in) {
        return {{"success", false}, {"error", "Cannot open file"}};
    }
    std::string bytes((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
    return {
        {"success", true},
        {"dataUrl", "data:" + mimeIt->second + ";base64," + Base64Encode(bytes)},
        {"fileSize", size}
    };
}

nlohmann::json CommandHandler::HandleFileSaveTemp(const nlohmann::json& args) {
    std::string base64Data = args.value("data", "");
    std::string filename = args.value("filename", "temp_file");

    if (base64Data.empty()) {
        return {{"success", false}, {"error", "Base64 data is empty"}};
    }

    // Decode base64
    std::string decodedData;
    try {
        // Simple base64 decode implementation
        static const std::string base64_chars =
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
            "abcdefghijklmnopqrstuvwxyz"
            "0123456789+/";

        int in_len = base64Data.size();
        int i = 0, j = 0;
        char char_array_4[4], char_array_3[3];

        while (in_len-- && base64Data[i] != '=' &&
               (isalnum(base64Data[i]) || base64Data[i] == '+' || base64Data[i] == '/')) {
            char_array_4[j++] = base64Data[i]; i++;
            if (j == 4) {
                for (j = 0; j < 4; j++)
                    char_array_4[j] = base64_chars.find(char_array_4[j]);
                char_array_3[0] = (char_array_4[0] << 2) + ((char_array_4[1] & 0x30) >> 4);
                char_array_3[1] = ((char_array_4[1] & 0xf) << 4) + ((char_array_4[2] & 0x3c) >> 2);
                char_array_3[2] = ((char_array_4[2] & 0x3) << 6) + char_array_4[3];
                for (j = 0; j < 3; j++)
                    decodedData += char_array_3[j];
                j = 0;
            }
        }

        if (j) {
            for (int k = j; k < 4; k++)
                char_array_4[k] = 0;
            for (int k = 0; k < 4; k++)
                char_array_4[k] = base64_chars.find(char_array_4[k]);
            char_array_3[0] = (char_array_4[0] << 2) + ((char_array_4[1] & 0x30) >> 4);
            char_array_3[1] = ((char_array_4[1] & 0xf) << 4) + ((char_array_4[2] & 0x3c) >> 2);
            char_array_3[2] = ((char_array_4[2] & 0x3) << 6) + char_array_4[3];
            for (int k = 0; k < j - 1; k++)
                decodedData += char_array_3[k];
        }
    } catch (...) {
        return {{"success", false}, {"error", "Failed to decode base64 data"}};
    }

    // Temp directory (%TEMP%\IPMsgPro). Everything here is UTF-8 and written
    // through the wide file API: the returned path is later opened by
    // StartSendFile via PathFromUtf8, so an ANSI %TEMP% (e.g. a Chinese user
    // name) mixed into a UTF-8 file name would never be found again.
    std::string tempDir = paths::AppTempDir();

    // Sanitize filename - remove path separators and special chars
    std::string safeFilename = SanitizeFileName(filename);

    // Generate unique filename
    std::string tempFile = tempDir + "\\" + std::to_string(std::time(nullptr)) + "_" + safeFilename;

    // Write to file
    std::ofstream outFile(enc::PathFromUtf8(tempFile), std::ios::binary);
    if (!outFile.is_open()) {
        DWORD err = GetLastError();
        return {{"success", false}, {"error", "Failed to create temp file: " + std::string(tempFile) + " err=" + std::to_string(err)}};
    }

    outFile.write(decodedData.data(), decodedData.size());
    outFile.close();

    return {{"success", true}, {"filePath", tempFile}};
}

nlohmann::json CommandHandler::HandleFileAccept(const nlohmann::json& args) {
    LogMessage("BRIDGE", "DEBUG", "[BACKEND-ACCEPT-ENTRY] file.accept called with args: " + args.dump());

    auto target = FindUserFromArgs(args);
    if (!target) {
        LogMessage("BRIDGE", "ERROR", "[BACKEND-ACCEPT-ERROR] Target user not found! args=" + args.dump());
        return {{"success", false}, {"error", "Target user not found"}};
    }
    LogMessage("BRIDGE", "DEBUG", "[BACKEND-ACCEPT]Found target user: " + target->Key() + ", ip=" + target->ipAddress + ", port=" + std::to_string(target->portNo));

    std::string transferId = args.value("transferId", "");
    std::string fileName = args.value("fileName", "");
    int64_t fileSize = args.value("fileSize", 0);
    std::string savePath = args.value("savePath", "");
    uint64_t origPacketNo = args.value("packetNo", (uint64_t)0);
    int origFileId = args.value("fileId", 0);

    // If savePath is empty, auto-generate using the user's Downloads folder.
    // The name comes from the peer: reduce it to a safe leaf name (no
    // directory components) and never overwrite an existing file.
    if (savePath.empty() && !fileName.empty()) {
        std::string saveDir = paths::UserDownloadsDir();
        CreateDirectoryW(Utf8ToWide(saveDir).c_str(), nullptr);
        savePath = UniqueSavePath(saveDir, SanitizeFileName(fileName));
    }
    LogMessage("BRIDGE", "DEBUG", "[BACKEND-ACCEPT]savePath=" + savePath);

    if (transferId.empty() || fileName.empty() || savePath.empty()) {
        LogMessage("BRIDGE", "ERROR", "[BACKEND-ACCEPT-ERROR] Missing required parameters!");
        return {{"success", false}, {"error", "Missing required parameters"}};
    }

    LogMessage("BRIDGE", "DEBUG", "[BACKEND-ACCEPT]Calling StartRecvFile: ip=" + target->ipAddress + ", port=" + std::to_string(target->portNo) + ", fileName=" + fileName + ", fileSize=" + std::to_string(fileSize) + ", savePath=" + savePath + ", origPacketNo=" + std::to_string(origPacketNo) + ", origFileId=" + std::to_string(origFileId));

    // Send IPMSG_RECVMSG acknowledgment to sender (delivery receipt)
    // Use the original SENDMSG packetNo (not the internal transferId)
    if (origPacketNo > 0) {
        msgMng_->SendRecvMsg(*target, origPacketNo);
    }

    // Normal mode: Start receiving file via TCP (same port as UDP per IPMsg protocol)
    // Pass origPacketNo and origFileId for building the GETFILEDATA request
    std::string recvTransferId = fileTransfer_->StartRecvFile(
        target->ipAddress, target->portNo, fileName, fileSize, savePath, target->Key(),
        origPacketNo, origFileId);

    LogMessage("BRIDGE", "DEBUG", "[BACKEND-ACCEPT]StartRecvFile result: recvTransferId=" + recvTransferId);

    if (recvTransferId.empty()) {
        LogMessage("BRIDGE", "ERROR", "[BACKEND-ACCEPT-ERROR] StartRecvFile failed!");
        return {{"success", false}, {"error", "Failed to start file receive"}};
    }

    // Save to database
    MessageRecord record;
    record.id = recvTransferId;
    record.fromId = target->Key();
    record.toId = msgMng_->GetLocalUser().Key();
    record.content = savePath;
    record.type = 2;  // file
    record.timestamp = static_cast<int64_t>(std::time(nullptr));
    record.status = kMsgStatusSending;  // completed/failed is written by the progress callback
    msgDb_->SaveMessage(record);

    // Emit transfer started event
    bridge_->Emit("file.transfer_started", {
        {"transferId", recvTransferId},
        {"filename", fileName},
        {"fileSize", fileSize},
        {"isSending", false},
        {"targetUser", target->Key()},
        {"savePath", savePath}
    });

    return {{"success", true}, {"transferId", recvTransferId}};
}

nlohmann::json CommandHandler::HandleFileReject(const nlohmann::json& args) {
    auto target = FindUserFromArgs(args);
    if (!target) {
        return {{"success", false}, {"error", "Target user not found"}};
    }

    std::string transferId = args.value("transferId", "");

    // Send IPMSG_RELEASEFILES to sender to release the shared files
    // In IPMsg protocol, RELEASEFILES uses the same packetNo as the original SENDMSG
    uint64_t packetNo = 0;
    try { packetNo = std::stoull(transferId); } catch (...) {}

    if (packetNo > 0) {
        // Send RELEASEFILES command to notify sender
        std::string extra = std::to_string(packetNo);
        msgMng_->SendMessage(*target, extra,
            IPMSG_RELEASEFILES | IPMSG_FILEATTACHOPT);
    }

    return {{"success", true}};
}

nlohmann::json CommandHandler::HandleShellOpen(const nlohmann::json& args) {
    std::string url = args.value("url", "");
    if (url.empty()) {
        return {{"success", false}, {"error", "URL is empty"}};
    }

    // Open the URL in the user's default browser via ShellExecuteW.
    // The URL is UTF-8; convert it to UTF-16 so it works regardless of locale.
    std::wstring wUrl = Utf8ToWide(url);
    if (wUrl.empty()) {
        return {{"success", false}, {"error", "Invalid URL encoding"}};
    }

    HINSTANCE result = ShellExecuteW(
        nullptr,
        L"open",
        wUrl.c_str(),
        nullptr,
        nullptr,
        SW_SHOWNORMAL
    );

    // ShellExecuteW returns > 32 on success
    bool ok = reinterpret_cast<INT_PTR>(result) > 32;
    return {{"success", ok}};
}

nlohmann::json CommandHandler::HandleScreenshotCapture(const nlohmann::json& args) {
    (void)args;
    if (!hwnd_) {
        return {{"success", false}, {"error", "Window handle not available"}};
    }
    HWND hWnd = static_cast<HWND>(hwnd_);

    // Determine the monitor that currently hosts the window (before we hide it),
    // so a multi-monitor setup captures the screen the app lives on.
    RECT winRect = { 0 };
    GetWindowRect(hWnd, &winRect);
    int cx = (winRect.left + winRect.right) / 2;
    int cy = (winRect.top + winRect.bottom) / 2;
    HMONITOR hMon = MonitorFromPoint({ cx, cy }, MONITOR_DEFAULTTONEAREST);
    if (!hMon) hMon = MonitorFromWindow(hWnd, MONITOR_DEFAULTTONEAREST);

    MONITORINFO mi = { sizeof(mi) };
    GetMonitorInfo(hMon, &mi);
    int screenCount = GetSystemMetrics(SM_CMONITORS);

    // Hide the window so it does not appear in the capture.
    if (window_) window_->Hide();

    // Give the OS a moment to repaint the desktop without our window.
    RedrawWindow(GetDesktopWindow(), NULL, NULL,
                 RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN);
    Sleep(220);

    std::vector<BYTE> png;
    bool ok = CaptureMonitorToPng(hMon, png);

    if (!ok) {
        // Bring the window back (keep its previous state).
        if (window_) window_->Restore();
        return {{"success", false}, {"error", "Failed to capture screen"}};
    }

    // Return the PNG as a data URL (base64). A data: URL is same-origin, so the
    // frontend can draw it to a canvas and read pixels back (required for
    // cropping / exporting) without tainting the canvas.
    std::string imageDataUrl = "data:image/png;base64," +
        Base64Encode(std::string((const char*)png.data(), png.size()));

    // Bring the window back, maximised, so the editor can go full-screen on the
    // same monitor the screenshot was taken from.
    if (window_) window_->Maximize();

    return {
        {"success", true},
        {"image", imageDataUrl},
        {"monitor", {
            {"x", mi.rcMonitor.left},
            {"y", mi.rcMonitor.top},
            {"width", mi.rcMonitor.right - mi.rcMonitor.left},
            {"height", mi.rcMonitor.bottom - mi.rcMonitor.top}
        }},
        {"screenCount", screenCount}
    };
}

nlohmann::json CommandHandler::HandleWindowMaximize(const nlohmann::json& args) {
    (void)args;
    if (window_) window_->Maximize();
    return {{"success", true}};
}

nlohmann::json CommandHandler::HandleWindowRestore(const nlohmann::json& args) {
    (void)args;
    if (window_) window_->Restore();
    return {{"success", true}};
}

nlohmann::json CommandHandler::HandleDialogSave(const nlohmann::json& args) {
    if (!hwnd_) {
        return {{"success", false}, {"error", "Window handle not available"}};
    }
    HWND hWnd = static_cast<HWND>(hwnd_);
    std::wstring title = Utf8ToWide(args.value("title", "保存截图"));
    std::wstring defaultName = Utf8ToWide(args.value("default_name", "screenshot.png"));

    OPENFILENAMEW ofn = { 0 };
    ofn.lStructSize = sizeof(ofn);
    ofn.hwndOwner = hWnd;
    ofn.lpstrFilter = L"PNG 图片 (*.png)\0*.png\0所有文件 (*.*)\0*.*\0";
    wchar_t szFile[MAX_PATH] = { 0 };
    wcsncpy_s(szFile, defaultName.c_str(), _TRUNCATE);
    ofn.lpstrFile = szFile;
    ofn.nMaxFile = MAX_PATH;
    ofn.lpstrTitle = title.c_str();
    ofn.Flags = OFN_OVERWRITEPROMPT | OFN_NOCHANGEDIR;

    if (GetSaveFileNameW(&ofn)) {
        std::string path = enc::WideToUtf8(std::wstring(szFile));
        return {{"success", true}, {"path", path}};
    }
    return {{"success", false}, {"cancelled", true}};
}

nlohmann::json CommandHandler::HandleFileSaveData(const nlohmann::json& args) {
    std::string base64Data = args.value("data", "");
    std::string path = args.value("path", "");
    if (base64Data.empty() || path.empty()) {
        return {{"success", false}, {"error", "Missing data or path"}};
    }
    std::string decoded = Base64Decode(base64Data);

    // The path is UTF-8 (from dialog.save); go through the wide API so a
    // Chinese folder name is not mangled by the ANSI code page.
    fs::path p = enc::PathFromUtf8(path);
    if (auto parent = p.parent_path(); !parent.empty()) {
        std::error_code ec;
        fs::create_directories(parent, ec);
    }

    {
        std::ofstream out(p, std::ios::binary);
        if (!out) {
            return {{"success", false}, {"error", "Cannot open target path"}};
        }
        out.write(decoded.data(), (std::streamsize)decoded.size());
        out.flush();
        if (!out) {
            return {{"success", false}, {"error", "Write failed"}};
        }
    }
    return {{"success", true}, {"path", path}};
}

nlohmann::json CommandHandler::HandleFileOpenFolder(const nlohmann::json& args) {
    std::string path = args.value("path", "");
    if (path.empty()) {
        return {{"success", false}, {"error", "Path is empty"}};
    }

    // Use ShellExecuteW to open explorer and select the file.
    // The path is UTF-8; convert it to UTF-16 properly so Chinese paths work.
    std::wstring wPath = Utf8ToWide(path);
    if (wPath.empty()) {
        return {{"success", false}, {"error", "Invalid path encoding"}};
    }
    std::wstring params = L"/select,\"" + wPath + L"\"";

    HINSTANCE result = ShellExecuteW(
        nullptr,
        L"open",
        L"explorer.exe",
        params.c_str(),
        nullptr,
        SW_SHOWNORMAL
    );

    // ShellExecuteW returns > 32 on success
    bool ok = reinterpret_cast<INT_PTR>(result) > 32;
    return {{"success", ok}};
}

// ---------- History Commands ----------

nlohmann::json CommandHandler::HandleHistoryGet(const nlohmann::json& args) {
    std::string userId = args.value("userId", "");
    int limit = args.value("limit", 50);
    int offset = args.value("offset", 0);

    // Get current user's ID
    std::string localUserId = msgMng_->GetLocalUser().Key();

    std::vector<MessageRecord> messages;
    bool ok = msgDb_->GetMessages(userId, localUserId, limit, offset, messages);

    nlohmann::json msgList = nlohmann::json::array();
    for (const auto& m : messages) {
        msgList.push_back({
            {"id", m.id},
            {"fromId", m.fromId},
            {"toId", m.toId},
            {"content", m.content},
            {"type", m.type},
            {"timestamp", m.timestamp},
            {"status", m.status}
        });
    }

    return {{"success", ok}, {"messages", msgList}, {"localUserId", localUserId}};
}

nlohmann::json CommandHandler::HandleHistorySearch(const nlohmann::json& args) {
    std::string keyword = args.value("keyword", "");
    if (keyword.empty()) {
        return {{"success", false}, {"error", "Keyword is empty"}};
    }

    std::vector<MessageRecord> messages;
    bool ok = msgDb_->SearchMessages(keyword, messages);

    nlohmann::json msgList = nlohmann::json::array();
    for (const auto& m : messages) {
        msgList.push_back({
            {"id", m.id},
            {"fromId", m.fromId},
            {"toId", m.toId},
            {"content", m.content},
            {"type", m.type},
            {"timestamp", m.timestamp},
            {"status", m.status}
        });
    }

    return {{"success", ok}, {"messages", msgList}};
}

nlohmann::json CommandHandler::HandleHistoryClear(const nlohmann::json& args) {
    std::string userId = args.value("userId", "");
    bool ok = msgDb_->ClearMessages(userId);
    return {{"success", ok}};
}

nlohmann::json CommandHandler::HandleHistoryGetRecent(const nlohmann::json& args) {
    int limit = args.value("limit", 20);
    if (limit <= 0) limit = 20;
    if (limit > 100) limit = 100;

    std::string localUserId = msgMng_->GetLocalUser().Key();

    std::vector<MessageRecord> messages;
    bool ok = msgDb_->GetRecentConversations(localUserId, limit, messages);

    nlohmann::json msgList = nlohmann::json::array();
    for (const auto& m : messages) {
        msgList.push_back({
            {"id", m.id},
            {"fromId", m.fromId},
            {"toId", m.toId},
            {"content", m.content},
            {"type", m.type},
            {"timestamp", m.timestamp},
            {"status", m.status}
        });
    }

    return {{"success", ok}, {"messages", msgList}, {"localUserId", localUserId}};
}

// ---------- Network Commands ----------

nlohmann::json CommandHandler::HandleNetworkScan(const nlohmann::json& args) {
    std::string segment = args.value("segment", "");
    if (!segment.empty()) {
        msgMng_->AddSegment(segment);
    }
    msgMng_->BroadcastEntry();
    return {{"success", true}};
}

nlohmann::json CommandHandler::HandleNetworkScanRange(const nlohmann::json& args) {
    // Support both old format (startIp/endIp) and new format (ranges array)
    std::vector<std::string> ranges;
    int port = args.value("port", msgMng_->GetLocalPort());
    int delayMs = args.value("delayMs", 50);

    if (args.contains("ranges") && args["ranges"].is_array()) {
        for (const auto& r : args["ranges"]) {
            if (r.is_string()) ranges.push_back(r.get<std::string>());
        }
    } else {
        // Backward compatibility
        std::string startIp = args.value("startIp", "");
        std::string endIp = args.value("endIp", "");
        if (!startIp.empty() && !endIp.empty()) {
            ranges.push_back(startIp + "-" + endIp);
        }
    }

    if (ranges.empty()) {
        return {{"success", false}, {"error", "ranges or startIp/endIp required"}};
    }

    // Scan progress/complete callbacks are registered once in
    // SetupEventForwarding(), so both manual and startup scans emit events.
    bool ok = msgMng_->ScanIpRanges(ranges, port, delayMs);
    return {{"success", ok}, {"message", ok ? "Scan started" : "Scan already in progress or invalid range"}};
}

nlohmann::json CommandHandler::HandleNetworkScanCancel(const nlohmann::json& args) {
    msgMng_->CancelScan();
    return {{"success", true}};
}

// ---------- Helpers ----------

nlohmann::json CommandHandler::UserToJson(const UserInfo& user) {
    std::string status = "online";
    if (!user.active) status = "offline";
    else if (user.hostStatus & IPMSG_ABSENCEOPT) status = "away";

    return {
        {"id", user.Key()},
        {"nickname", user.nickName.empty() ? user.userName : user.nickName},
        {"username", user.userName},
        {"hostname", user.hostName},
        {"group", user.groupName},
        {"ip", user.ipAddress},
        {"port", user.portNo},
        {"status", status},
        {"version", ""}
    };
}

std::optional<UserInfo> CommandHandler::FindUserFromArgs(const nlohmann::json& args) {
    // Try finding by "target" (key or IP)
    if (args.contains("target")) {
        std::string target = args["target"].get<std::string>();

        // Try as key first
        auto user = msgMng_->FindUser(target);
        if (user) return user;

        // Try as IP address
        auto users = msgMng_->GetUsers();
        for (const auto& u : users) {
            if (u.ipAddress == target) return u;
        }
    }
    return std::nullopt;
}

void CommandHandler::RegisterPendingAck(uint64_t packetNo, const std::string& messageId) {
    std::lock_guard<std::mutex> lk(pendingAcksMutex_);
    const auto now = std::chrono::steady_clock::now();
    // Peers that never answer (offline, or clients without SENDCHECKOPT
    // support) would otherwise grow the map without bound.
    for (auto it = pendingAcks_.begin(); it != pendingAcks_.end();) {
        if (now - it->second.sentAt > std::chrono::seconds(kPendingAckMaxAgeSec)) {
            it = pendingAcks_.erase(it);
        } else {
            ++it;
        }
    }
    pendingAcks_[packetNo] = PendingAck{messageId, now};
}

std::string CommandHandler::TakePendingAck(uint64_t packetNo) {
    std::lock_guard<std::mutex> lk(pendingAcksMutex_);
    auto it = pendingAcks_.find(packetNo);
    if (it == pendingAcks_.end()) return {};
    std::string id = std::move(it->second.messageId);
    pendingAcks_.erase(it);
    return id;
}

// ---------- Dialog Commands ----------

nlohmann::json CommandHandler::HandleDialogPickFolder(const nlohmann::json& args) {
    std::string title = args.value("title", "Select Folder");
    std::string initialDir = args.value("initial_dir", "");
    LogMessage("BRIDGE", "DEBUG", "[DIALOG]HandleDialogPickFolder called, title=" + title +
               ", initialDir=" + (initialDir.empty() ? "(default)" : initialDir));

    if (!hwnd_) {
        LogMessage("BRIDGE", "ERROR", "[DIALOG] hwnd_ is null!");
        return {{"success", false}, {"error", "Window handle not available"}};
    }

    LogMessage("BRIDGE", "DEBUG", "[DIALOG]hwnd_=" + std::to_string(reinterpret_cast<uintptr_t>(hwnd_)));
    HWND hWnd = static_cast<HWND>(hwnd_);
    LogMessage("BRIDGE", "DEBUG", "[DIALOG]Calling PickFolder with hWnd=" + std::to_string(reinterpret_cast<uintptr_t>(hWnd)));

    auto folder = tauricpp::Dialog::PickFolder(hWnd, title, initialDir);
    LogMessage("BRIDGE", "DEBUG", "[DIALOG]PickFolder returned, folder=" + (folder ? *folder : "(empty)"));
    
    if (folder) {
        return {{"success", true}, {"folder", *folder}};
    }
    return {{"success", true}, {"folder", ""}};  // User cancelled
}

nlohmann::json CommandHandler::HandleDialogOpen(const nlohmann::json& args) {
    std::string title = args.value("title", "选择文件");
    bool multi = args.value("multi_select", false);
    LogMessage("BRIDGE", "DEBUG", "[DIALOG]HandleDialogOpen called, title=" + title);

    if (!hwnd_) {
        LogMessage("BRIDGE", "ERROR", "[DIALOG] hwnd_ is null!");
        return {{"success", false}, {"error", "Window handle not available"}};
    }

    HWND hWnd = static_cast<HWND>(hwnd_);
    tauricpp::Dialog::OpenOptions opts;
    opts.title = title;
    opts.multi_select = multi;
    if (args.contains("filters") && args["filters"].is_array()) {
        for (const auto& filter : args["filters"]) {
            if (filter.is_object() && filter.contains("name") && filter["name"].is_string() &&
                filter.contains("pattern") && filter["pattern"].is_string()) {
                opts.filters.push_back({filter["name"].get<std::string>(), filter["pattern"].get<std::string>()});
            }
        }
    }
    if (args.contains("default_path") && args["default_path"].is_string()) {
        opts.default_path = args["default_path"].get<std::string>();
    }

    // 返回 UTF-8 路径，后端的 StartSendFile 用 fs::u8path 正确解析中文路径
    auto files = tauricpp::Dialog::OpenFile(hWnd, opts);
    nlohmann::json arr = nlohmann::json::array();
    for (const auto& f : files) arr.push_back(f);
    return {{"success", true}, {"files", arr}};
}


std::string CommandHandler::GetDataDir() const {
    // Same rule as startup (paths::ResolveDataDir): custom dir from this
    // session or the registry, else %USERPROFILE%\.speedipmsg, plus the port
    // suffix so a second instance on another port keeps its own data.
    const int port = msgMng_ ? msgMng_->GetLocalPort() : IPMSG_DEFAULT_PORT;
    if (!dataDir_.empty()) {
        return paths::ApplyPortSuffix(dataDir_, port);
    }
    return paths::ResolveDataDir(port);
}

void CommandHandler::DumpDebugFile(const std::string& fileName, const std::string& data) const {
    if (!IsDebugEnabled()) return;
    const std::string dir = GetDataDir() + "\\debug";
    std::error_code ec;
    fs::create_directories(enc::PathFromUtf8(dir), ec);
    std::ofstream out(enc::PathFromUtf8(dir + "\\" + fileName), std::ios::binary);
    if (out) out.write(data.data(), static_cast<std::streamsize>(data.size()));
}

// Surface a reassembled FeiQ screenshot to the frontend as a finished image
// message (inline base64 data URL), bypassing the "accept file" UI entirely.
void CommandHandler::EmitFeiQScreenshot(const FeiQScreenshotResult& shot) {
    const std::string messageId = "feiq_" + shot.sender.Key() + ":" + shot.sender.ipAddress + ":" +
        std::to_string(shot.sender.portNo) + ":" + shot.id;
    MessageRecord record;
    record.id = messageId;
    record.fromId = shot.sender.Key();
    record.toId = msgMng_->GetLocalUser().Key();
    record.content = shot.savePath;
    record.type = 1;
    record.timestamp = std::time(nullptr);
    record.status = kMsgStatusDelivered;
    if (!msgDb_->SaveMessage(record)) LogMessage("IMAGE", "ERROR", "Could not save received image history");
    std::string mime = (shot.ext == "png") ? "png" : (shot.ext == "jpg" ? "jpeg" : "bmp");
    std::string dataUrl = "data:image/" + mime + ";base64," + Base64Encode(shot.bytes);
    bridge_->Emit("feiq.screenshot_received", {
        {"messageId", messageId}, {"imageId", shot.id}, {"timestamp", record.timestamp},
        {"fromUser", UserToJson(shot.sender)},
        {"dataUrl", dataUrl},
        {"fileName", "\xe9\xa3\x9e\xe7\xa7\x8b\xe6\x88\xaa\xe5\x9b\xbe_" + shot.id + "." + shot.ext},
        {"savePath", shot.savePath},
        {"fileSize", static_cast<int64_t>(shot.bytes.size())}
    });
    NotifyIncoming(shot.sender, "[图片]");
}

} // namespace ipmsg
