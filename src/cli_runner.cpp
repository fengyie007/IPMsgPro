// ============================================================================
// Headless CLI mode (--mode=cli): a protocol test harness.
//   --cmd=server  auto-accepts every incoming file and echoes text messages
//   --cmd=test    sends text/image/file to --target from a JSON config
// Kept out of main.cpp so the GUI entry point stays small. The file-level
// pointers below mirror the globals the harness used to share with WinMain.
// ============================================================================
#include "cli_runner.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <WinSock2.h>
#include <Windows.h>

#include "ipmsg/msgmng.h"
#include "file/file_transfer.h"
#include "logger.h"
#include "util/app_paths.h"
#include "util/encoding.h"

#include <atomic>
#include <chrono>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <string>
#include <thread>
#include <vector>
#include <nlohmann/json.hpp>

namespace ipmsg {
namespace cli {

using ipmsg::enc::Utf8ToWide;

namespace {

void Log(const std::string& level, const std::string& msg) {
    LogMessage("CLI", level, msg);
}

MsgMng* g_msgMng = nullptr;
FileTransferManager* g_fileTransfer = nullptr;

}  // namespace

#define LOG_INFO(msg) Log("INFO", msg)
#define LOG_WARN(msg) Log("WARN", msg)
#define LOG_ERROR(msg) Log("ERROR", msg)
#define LOG_DEBUG(msg) Log("DEBUG", msg)

// ============================================================================
// Helper: Parse colon-separated file attachment fields (:: escaped colons)
// ============================================================================
static bool ParseFileAttachExtra(const std::string& extra, int& outFileId,
                                  std::string& outFileName, int64_t& outFileSize) {
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
                current += ':';
                i++;
            } else {
                fields.push_back(current);
                current.clear();
            }
        } else {
            current += fileInfo[i];
        }
    }
    fields.push_back(current);

    if (fields.size() < 3) return false;

    try { outFileId = std::stoi(fields[0]); } catch (...) {}
    outFileName = fields[1];
    try { outFileSize = std::stoll(fields[2], nullptr, 16); } catch (...) {}
    return true;
}

// ============================================================================
// CLI Mode - Server (auto-accept all file transfers)
// ============================================================================
static void RunCliServer(int port) {
    LOG_INFO("CLI SERVER mode on port " + std::to_string(port));
    LOG_INFO("Waiting for file transfers... Auto-accept enabled.");

    // Set up progress callback
    g_fileTransfer->SetProgressCallback(
        [](const ipmsg::TransferProgress& progress) {
            if (progress.status == ipmsg::TransferStatus::Completed) {
                std::string dir = progress.isSending ? "Sent" : "Received";
                LOG_INFO("[TRANSFER " + dir + "] " + progress.filename +
                         " (" + std::to_string(progress.fileSize) + " bytes)" +
                         " path=" + progress.localPath);
            } else if (progress.status == ipmsg::TransferStatus::Failed) {
                LOG_ERROR("[TRANSFER FAILED] " + progress.filename);
            } else if (progress.status == ipmsg::TransferStatus::Transferring) {
                int pct = progress.fileSize > 0
                    ? (int)(progress.transferred * 100 / progress.fileSize)
                    : 0;
                LOG_DEBUG("[TRANSFER PROGRESS] " + progress.filename +
                          " " + std::to_string(pct) + "%");
            }
        }
    );

    // Watch for incoming messages with file attachments and auto-accept
    g_msgMng->SetMessageReceivedCallback(
        [](const ipmsg::MsgBuf& msg) {
            char cmdBuf[32] = {};
            snprintf(cmdBuf, sizeof(cmdBuf), "0x%08lx", (unsigned long)msg.command);
            LOG_INFO("[RECV] from " + msg.sender.userName + "@" +
                     msg.sender.ipAddress + " cmd=" + cmdBuf);

            uint32_t mode = msg.command & 0x000000ff;
            if (mode == 0x20 && (msg.command & 0x00200000)) {
                // SENDMSG | FILEATTACHOPT - auto-accept
                LOG_INFO("[FILE NOTIFICATION] from " + msg.sender.userName +
                         " packetNo=" + std::to_string(msg.packetNo));

                // Reply RECVMSG
                g_msgMng->SendRecvMsg(msg.sender, msg.packetNo);

                // Parse file info from extra
                int fileId = 0;
                std::string fileName;
                int64_t fileSize = 0;
                if (ParseFileAttachExtra(msg.extra, fileId, fileName, fileSize)) {
                    LOG_INFO("[AUTO-ACCEPT] File: " + fileName + " (" +
                             std::to_string(fileSize) + " bytes)");

                    // Generate save path
                    std::string saveDir = ipmsg::paths::UserDownloadsDir();
                    CreateDirectoryW(Utf8ToWide(saveDir).c_str(), nullptr);
                    std::string savePath = saveDir + "\\" + fileName;

                    // Start receiving - use correct sender port
                    // The msg.sender.portNo may be a UDP ephemeral port; try to find the
                    // actual listening port from the user list.
                    int senderPort = msg.sender.portNo;
                    auto knownUser = g_msgMng->FindUser(msg.sender.Key());
                    if (knownUser && knownUser->portNo != msg.sender.portNo) {
                        LOG_INFO("[AUTO-ACCEPT] Using known port " + std::to_string(knownUser->portNo) +
                                 " instead of UDP source port " + std::to_string(msg.sender.portNo));
                        senderPort = knownUser->portNo;
                    }
                    std::string recvTransferId = g_fileTransfer->StartRecvFile(
                        msg.sender.ipAddress, senderPort,
                        fileName, fileSize, savePath,
                        msg.sender.Key(), msg.packetNo, fileId);

                    if (!recvTransferId.empty()) {
                        LOG_INFO("[AUTO-ACCEPT] Transfer started: " + recvTransferId);
                    } else {
                        LOG_ERROR("[AUTO-ACCEPT] Failed to start transfer");
                    }
                }
            } else if (mode == 0x20) {
                // Text message received
                LOG_INFO("[TEXT] from " + msg.sender.userName + "@" +
                         msg.sender.ipAddress + ":" + std::to_string(msg.sender.portNo) + ": " + msg.body);

                // Reply RECVMSG
                g_msgMng->SendRecvMsg(msg.sender, msg.packetNo);
                
                // Echo: send back the message with "Echo:" prefix
                // But don't echo messages that already start with "Echo:" (loop prevention)
                if (msg.body.substr(0, 5) != "Echo:") {
                    g_msgMng->SendMessage(msg.sender, "Echo:" + msg.body);
                    LOG_INFO("[ECHO] sent to " + msg.sender.userName + "@" +
                             msg.sender.ipAddress + ":" + std::to_string(msg.sender.portNo) + ": " + msg.body);
                }
            }
        }
    );

    // Watch for user discovery
    g_msgMng->SetUserDiscoveredCallback(
        [](const ipmsg::UserInfo& user) {
            LOG_INFO("[USER] Discovered: " + user.userName + "@" +
                     user.ipAddress + ":" + std::to_string(user.portNo));
        }
    );

    LOG_INFO("CLI Server ready. Listening on port " + std::to_string(port));

    // Keep running until Ctrl+C
    HANDLE hEvent = CreateEventA(NULL, TRUE, FALSE, NULL);
    SetConsoleCtrlHandler([](DWORD) -> BOOL {
        LOG_INFO("Shutdown signal received");
        return FALSE;
    }, TRUE);

    // Periodically broadcast entry
    while (true) {
        g_msgMng->BroadcastEntry();
        WaitForSingleObject(hEvent, 30000); // broadcast every 30s
        // Check if handler set shutdown
        if (WaitForSingleObject(hEvent, 0) == WAIT_OBJECT_0) break;
    }
}

// ============================================================================
// CLI Mode - Test Runner (send text/image/file from JSON config)
// ============================================================================
struct TestConfig {
    int targetPort = 2425;
    std::string targetIp = "127.0.0.1";
    std::vector<nlohmann::json> testItems;
};

static TestConfig ParseTestConfig(const std::string& configPath) {
    TestConfig config;
    std::ifstream f(configPath);
    if (!f.is_open()) {
        LOG_ERROR("Cannot open config file: " + configPath);
        return config;
    }
    nlohmann::json j;
    f >> j;

    if (j.contains("target_ip")) config.targetIp = j["target_ip"];
    if (j.contains("target_port")) config.targetPort = j["target_port"];
    if (j.contains("tests") && j["tests"].is_array()) {
        config.testItems = j["tests"].get<std::vector<nlohmann::json>>();
    }
    return config;
}

static void RunCliTestRunner(int port, const std::string& configPath,
                              const std::string& targetIp, int targetPort) {
    LOG_INFO("CLI TEST RUNNER mode on port " + std::to_string(port));
    LOG_INFO("Target: " + targetIp + ":" + std::to_string(targetPort));

    // Load test config
    TestConfig config;
    if (!configPath.empty()) {
        config = ParseTestConfig(configPath);
        if (config.targetIp.empty()) config.targetIp = targetIp;
        if (config.targetPort == 0) config.targetPort = targetPort;
    } else {
        config.targetIp = targetIp;
        config.targetPort = targetPort;
    }

    LOG_INFO("Test target: " + config.targetIp + ":" + std::to_string(config.targetPort));

    // Set up progress callback for file transfers
    g_fileTransfer->SetProgressCallback(
        [](const ipmsg::TransferProgress& progress) {
            if (progress.status == ipmsg::TransferStatus::Completed) {
                std::string dir = progress.isSending ? "Sent" : "Received";
                if (progress.isSending) {
                    LOG_INFO("[SEND OK] " + progress.filename +
                             " (" + std::to_string(progress.fileSize) + " bytes)");
                }
            } else if (progress.status == ipmsg::TransferStatus::Failed) {
                LOG_ERROR("[SEND FAILED] " + progress.filename);
            } else if (progress.status == ipmsg::TransferStatus::Transferring && progress.isSending) {
                int pct = progress.fileSize > 0
                    ? (int)(progress.transferred * 100 / progress.fileSize)
                    : 0;
                LOG_DEBUG("[SEND PROGRESS] " + progress.filename + " " +
                          std::to_string(pct) + "%");
            }
        }
    );

    // Watch for incoming messages
    g_msgMng->SetMessageReceivedCallback(
        [](const ipmsg::MsgBuf& msg) {
            uint32_t mode = msg.command & 0x000000ff;
            if (mode == 0x20 && (msg.command & 0x00200000)) {
                // SENDMSG | FILEATTACHOPT - auto-accept for images
                LOG_INFO("[RECV FILE NOTIFY] from " + msg.sender.userName +
                         " packetNo=" + std::to_string(msg.packetNo));
                g_msgMng->SendRecvMsg(msg.sender, msg.packetNo);

                int fileId = 0;
                std::string fileName;
                int64_t fileSize = 0;
                if (ParseFileAttachExtra(msg.extra, fileId, fileName, fileSize)) {
                    LOG_INFO("[RECV FILE] " + fileName + " (" +
                             std::to_string(fileSize) + " bytes)");

                    std::string saveDir = ipmsg::paths::UserDownloadsDir();
                    CreateDirectoryW(Utf8ToWide(saveDir).c_str(), nullptr);
                    std::string savePath = saveDir + "\\" + fileName;

                    std::string recvTransferId = g_fileTransfer->StartRecvFile(
                        msg.sender.ipAddress, msg.sender.portNo,
                        fileName, fileSize, savePath,
                        msg.sender.Key(), msg.packetNo, fileId);

                    if (!recvTransferId.empty()) {
                        LOG_INFO("[RECV STARTED] " + recvTransferId);
                    }
                }
            } else if (mode == 0x20 && !(msg.command & 0x00200000)) {
                // Text message received
                g_msgMng->SendRecvMsg(msg.sender, msg.packetNo);
                LOG_INFO("[RECV TEXT] from " + msg.sender.userName + "@" +
                         msg.sender.ipAddress + ": " + msg.body);
            } else if (mode == 0x21) {
                LOG_INFO("[RECVMSG ACK] from " + msg.sender.userName +
                         " pkt=" + msg.body);
            }
        }
    );

    // Watch for user discovery
    std::atomic<bool> targetDiscovered{false};
    g_msgMng->SetUserDiscoveredCallback(
        [&](const ipmsg::UserInfo& user) {
            LOG_INFO("[USER] Discovered: " + user.userName + "@" +
                     user.ipAddress + ":" + std::to_string(user.portNo));
            if (user.ipAddress == config.targetIp ||
                (config.targetIp == "127.0.0.1" && user.ipAddress != "0.0.0.0")) {
                targetDiscovered.store(true);
            }
        }
    );

    // Discover target user - broadcast BR_ENTRY
    LOG_INFO("Sending BR_ENTRY to discover target...");
    g_msgMng->BroadcastEntry();

    // Wait up to 5 seconds for target discovery
    for (int i = 0; i < 50; i++) {
        if (targetDiscovered.load()) {
            LOG_INFO("Target discovered!");
            break;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(100));
    }

    if (!targetDiscovered.load()) {
        LOG_WARN("Target not discovered via broadcast, will try direct send anyway");
    }

    // Find the target user
    auto target = g_msgMng->FindUser(config.targetIp);
    if (!target) {
        // Try by ip:port
        auto users = g_msgMng->GetUsers();
        for (const auto& u : users) {
            if (u.ipAddress == config.targetIp || u.portNo == config.targetPort) {
                target = u;
                break;
            }
        }
    }

    if (!target) {
        // Create virtual target for direct send
        ipmsg::UserInfo virtTarget;
        virtTarget.userName = "TestServer";
        virtTarget.hostName = "SERVER-PC";
        virtTarget.nickName = "TestServer";
        virtTarget.ipAddress = config.targetIp;
        virtTarget.portNo = config.targetPort;
        virtTarget.active = true;
        LOG_INFO("Using virtual target: " + virtTarget.userName + "@" +
                 virtTarget.ipAddress + ":" + std::to_string(virtTarget.portNo));
        target = virtTarget;
    } else {
        LOG_INFO("Found target: " + target->userName + "@" +
                 target->ipAddress + ":" + std::to_string(target->portNo));
    }

    std::this_thread::sleep_for(std::chrono::milliseconds(500));

    // ============================================================
    // Execute tests
    // ============================================================
    if (!config.testItems.empty()) {
        LOG_INFO("=== Running " + std::to_string(config.testItems.size()) + " tests from config ===");
        for (size_t idx = 0; idx < config.testItems.size(); idx++) {
            const auto& item = config.testItems[idx];
            std::string type = item.value("type", "text");
            std::string content = item.value("content", "");

            LOG_INFO("--- Test " + std::to_string(idx + 1) + "/" +
                     std::to_string(config.testItems.size()) +
                     ": type=" + type + " ---");

            if (type == "text") {
                // Send text message
                bool ok = g_msgMng->SendMessage(*target, content);
                if (ok) {
                    LOG_INFO("[SEND TEXT] \"" + content + "\" -> OK");
                } else {
                    LOG_ERROR("[SEND TEXT] FAILED");
                }
            } else if (type == "file" || type == "image") {
                // Send file/image
                std::string filePath = content;
                if (filePath.empty()) {
                    LOG_ERROR("[SEND FILE] No file path specified");
                    continue;
                }

                // Check file exists (paths from the JSON config are UTF-8)
                std::error_code ec;
                const auto fsPath = enc::PathFromUtf8(filePath);
                if (!std::filesystem::is_regular_file(fsPath, ec)) {
                    LOG_ERROR("[SEND FILE] File not found: " + filePath);
                    continue;
                }

                // Get file size
                int64_t fileSize = static_cast<int64_t>(std::filesystem::file_size(fsPath, ec));
                if (ec) fileSize = 0;

                // Get file name
                auto lastSep = filePath.find_last_of("/\\");
                std::string fileName = (lastSep != std::string::npos)
                    ? filePath.substr(lastSep + 1) : filePath;

                // Register file for transfer
                std::string transferId = g_fileTransfer->StartSendFile(
                    target->ipAddress, target->portNo, filePath, target->Key());

                if (transferId.empty()) {
                    LOG_ERROR("[SEND FILE] Failed to register transfer");
                    continue;
                }

                // Get file info to get fileId
                auto fileInfo = g_fileTransfer->GetFileInfo(transferId);
                if (!fileInfo) {
                    LOG_ERROR("[SEND FILE] Failed to get file info");
                    continue;
                }

                // Build file attach info in Feiq format
                std::ostringstream attachOs;
                {
                    std::string escapedFileName = fileName;
                    // Escape colons
                    std::string escaped;
                    for (char c : escapedFileName) {
                        if (c == ':') escaped += "::";
                        else escaped += c;
                    }
                    escapedFileName = escaped;
                    attachOs << fileInfo->fileId << ":" << escapedFileName << ":"
                             << std::hex << fileSize << ":"
                             << 0 << ":"  // modify time
                             << 0 << ":\x07";  // file type
                }
                std::string fileAttachInfo = attachOs.str();

                // Send UDP notification
                uint64_t sentPktNo = g_msgMng->SendMessageWithFile(
                    *target, "[File: " + fileName + "]", fileAttachInfo);

                if (sentPktNo > 0) {
                    // Store packetNo for matching GETFILEDATA
                    auto fi = g_fileTransfer->GetFileInfo(transferId);
                    if (fi) {
                        fi->packetNo = sentPktNo;
                        g_fileTransfer->RegisterFileInfo(transferId, *fi);
                    }
                    LOG_INFO("[SEND FILE NOTIFY] " + fileName + " (" +
                             std::to_string(fileSize) + " bytes) pktNo=" +
                             std::to_string(sentPktNo) + " transferId=" + transferId);
                    LOG_INFO("[SEND FILE] Waiting for RECVMSG + TCP transfer...");
                } else {
                    LOG_ERROR("[SEND FILE NOTIFY] UDP send failed");
                }
            } else {
                LOG_WARN("[TEST] Unknown type: " + type);
            }

            // Wait between tests
            if (idx + 1 < config.testItems.size()) {
                int delay = item.value("delay_ms", 2000);
                std::this_thread::sleep_for(std::chrono::milliseconds(delay));
            }
        }
    }

    // Keep running a bit to let file transfers complete
    LOG_INFO("Waiting for transfers to complete...");
    std::this_thread::sleep_for(std::chrono::seconds(10));

    LOG_INFO("=== CLI TEST RUNNER COMPLETE ===");
}

int Run(const std::string& subCmd, int port, const std::string& configPath,
        const std::string& targetIp, int targetPort,
        MsgMng& msgMng, FileTransferManager& fileTransfer) {
    g_msgMng = &msgMng;
    g_fileTransfer = &fileTransfer;

    if (subCmd == "server") {
        RunCliServer(port);
    } else if (subCmd == "test") {
        RunCliTestRunner(port, configPath, targetIp, targetPort);
    } else {
        LOG_ERROR("Unknown CLI command: " + subCmd);
        LOG_INFO("Usage: SpeedIpMsg.exe --mode=cli --cmd=server|test --port=PORT [--config=<json>] [--target=ip[:port]]");
        return 2;
    }
    return 0;
}

#undef LOG_INFO
#undef LOG_WARN
#undef LOG_ERROR
#undef LOG_DEBUG

}  // namespace cli
}  // namespace ipmsg
