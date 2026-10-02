// ============================================================================
// IPMsgPro Application Entry Point
// ============================================================================

// WinSock2 must come before Windows.h
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <WinSock2.h>
#include <WS2tcpip.h>
#include <Windows.h>

#include <tauricpp/app.hpp>
#include <tauricpp/dialog.hpp>

#include "bridge/command_handler.h"
#include "cli_runner.h"
#include "ipmsg/msgmng.h"
#include "ipmsg/network.h"
#include "database/message_db.h"
#include "file/file_transfer.h"
#include "util/app_paths.h"
#include "util/encoding.h"

#include <string>
#include <atomic>
#include <exception>
#include <cstring>
#include <iostream>
#include <fstream>
#include <chrono>
#include <iomanip>
#include <sstream>
#include <thread>
#include <nlohmann/json.hpp>

// ============================================================================
// Logger (unified into ipmsg_gui_debug.log via logger.h)
// ============================================================================
#include "logger.h"

static void Log(const std::string& level, const std::string& msg) {
    ipmsg::LogMessage("IPMSGPRO", level, msg);
}

// 应用版本号（与 CMakeLists.txt / resources/app.rc 保持一致）
static const char* kAppVersion = "1.5.0";

#define LOG_INFO(msg) Log("INFO", msg)
#define LOG_WARN(msg) Log("WARN", msg)
#define LOG_ERROR(msg) Log("ERROR", msg)
#define LOG_DEBUG(msg) Log("DEBUG", msg)

// Global components (managed by main)
static ipmsg::MsgMng* g_msgMng = nullptr;
static ipmsg::MessageDB* g_msgDb = nullptr;
static ipmsg::FileTransferManager* g_fileTransfer = nullptr;

// All paths in this file are UTF-8; convert at the Win32 boundary.
using ipmsg::enc::WideToUtf8;

// ============================================================================
// Parse command line arguments
// ============================================================================
struct CliArgs {
    int port = ipmsg::IPMSG_DEFAULT_PORT;
    bool cliMode = false;
    bool verbose = false;      // --verbose: write DEBUG-level log lines
    std::string subCmd;        // "server" or "test"
    std::string configPath;    // path to JSON test config
    std::string targetIp = "127.0.0.1";
    int targetPort = 2425;

    // Users to auto-add in GUI mode (format: "ip:port" or "ip[:port][,ip2:port2]")
    std::vector<std::string> addUsers;
};

static CliArgs ParseCommandLine(LPSTR lpCmdLine) {
    CliArgs args;
    std::string cmdLine = lpCmdLine;

    // Parse --port
    size_t portPos = cmdLine.find("--port=");
    if (portPos != std::string::npos) {
        size_t start = portPos + 7;
        size_t end = cmdLine.find(' ', start);
        std::string portStr = (end != std::string::npos)
            ? cmdLine.substr(start, end - start)
            : cmdLine.substr(start);
        args.port = std::stoi(portStr);
    } else {
        portPos = cmdLine.find("--port ");
        if (portPos != std::string::npos) {
            size_t start = portPos + 7;
            while (start < cmdLine.size() && cmdLine[start] == ' ') ++start;
            size_t end = cmdLine.find(' ', start);
            std::string portStr = (end != std::string::npos)
                ? cmdLine.substr(start, end - start)
                : cmdLine.substr(start);
            args.port = std::stoi(portStr);
        }
    }

    // Parse --mode=cli
    if (cmdLine.find("--mode=cli") != std::string::npos) {
        args.cliMode = true;
    }

    // Parse --verbose (DEBUG log level)
    if (cmdLine.find("--verbose") != std::string::npos ||
        cmdLine.find("--log-level=debug") != std::string::npos) {
        args.verbose = true;
    }

    // Parse --cmd=server or --cmd=test
    size_t cmdPos = cmdLine.find("--cmd=");
    if (cmdPos != std::string::npos) {
        size_t start = cmdPos + 6;
        size_t end = cmdLine.find(' ', start);
        args.subCmd = (end != std::string::npos)
            ? cmdLine.substr(start, end - start)
            : cmdLine.substr(start);
    }

    // Parse --config=<path>
    size_t cfgPos = cmdLine.find("--config=");
    if (cfgPos != std::string::npos) {
        size_t start = cfgPos + 9;
        size_t end = cmdLine.find(' ', start);
        args.configPath = (end != std::string::npos)
            ? cmdLine.substr(start, end - start)
            : cmdLine.substr(start);
    }

    // Parse --target=<ip>
    size_t tgtPos = cmdLine.find("--target=");
    if (tgtPos != std::string::npos) {
        size_t start = tgtPos + 9;
        size_t end = cmdLine.find(' ', start);
        std::string targetStr = (end != std::string::npos)
            ? cmdLine.substr(start, end - start)
            : cmdLine.substr(start);
        // Format: "ip:port" or just "ip"
        size_t colonPos = targetStr.find(':');
        if (colonPos != std::string::npos) {
            args.targetIp = targetStr.substr(0, colonPos);
            args.targetPort = std::stoi(targetStr.substr(colonPos + 1));
        } else {
            args.targetIp = targetStr;
        }
    }

    // Parse --adduser=<ip:port>[,<ip2:port2>,...]
    size_t addPos = cmdLine.find("--adduser=");
    if (addPos != std::string::npos) {
        size_t start = addPos + 10;
        size_t end = cmdLine.find(' ', start);
        std::string userStr = (end != std::string::npos)
            ? cmdLine.substr(start, end - start)
            : cmdLine.substr(start);
        // Split by comma
        size_t commaPos = 0;
        while (commaPos < userStr.size()) {
            size_t nextComma = userStr.find(',', commaPos);
            std::string entry = (nextComma != std::string::npos)
                ? userStr.substr(commaPos, nextComma - commaPos)
                : userStr.substr(commaPos);
            if (!entry.empty()) {
                args.addUsers.push_back(entry);
            }
            if (nextComma == std::string::npos) break;
            commaPos = nextComma + 1;
        }
    }

    return args;
}

// ============================================================================
// System info logging at startup (version / OS / locale / IP)
// ============================================================================
static std::string GetWindowsVersionString() {
    std::string result;
    HKEY hKey = nullptr;
    // 读取 HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion 获取系统版本
    if (RegOpenKeyExW(HKEY_LOCAL_MACHINE,
                      L"SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
                      0, KEY_READ, &hKey) == ERROR_SUCCESS) {
        auto readStr = [&](LPCWSTR name) -> std::wstring {
            wchar_t buf[512] = {};
            DWORD size = sizeof(buf);
            if (RegQueryValueExW(hKey, name, nullptr, nullptr, (LPBYTE)buf, &size) == ERROR_SUCCESS)
                return std::wstring(buf);
            return L"";
        };
        std::wstring product = readStr(L"ProductName");     // 如 "Windows 10 Pro"
        std::wstring curVer  = readStr(L"CurrentVersion");  // 如 "10.0"
        std::wstring build   = readStr(L"CurrentBuild");    // 如 "19045"
        std::wstring disp    = readStr(L"DisplayVersion");  // 如 "22H2"
        if (disp.empty()) disp = readStr(L"ReleaseId");     // 旧系统回退
        result = WideToUtf8(product);
        std::string ver = WideToUtf8(curVer);
        if (!ver.empty()) result += " " + ver;
        std::string buildU = WideToUtf8(build);
        if (!buildU.empty()) result += "." + buildU;
        std::string dispU = WideToUtf8(disp);
        if (!dispU.empty()) result += " (" + dispU + ")";
        RegCloseKey(hKey);
    }
    return result.empty() ? "Unknown" : result;
}

static std::string GetDefaultLocaleString() {
    wchar_t localeName[LOCALE_NAME_MAX_LENGTH] = {};
    // 返回形如 "zh-CN" 的默认用户区域（语言-地区）
    if (GetUserDefaultLocaleName(localeName, LOCALE_NAME_MAX_LENGTH) > 0) {
        return WideToUtf8(std::wstring(localeName));
    }
    return "Unknown";
}

static void LogSystemInfo() {
    LOG_INFO("App Version: " + std::string(kAppVersion));
    LOG_INFO("OS Version: " + GetWindowsVersionString());
    LOG_INFO("Default Locale (language/region): " + GetDefaultLocaleString());

    auto ips = ipmsg::GetLocalIPAddresses();
    if (ips.empty()) {
        LOG_INFO("Local IP: (none)");
    } else {
        std::string joined;
        for (size_t i = 0; i < ips.size(); ++i) {
            joined += ips[i];
            if (i + 1 < ips.size()) joined += ", ";
        }
        LOG_INFO("Local IP addresses: " + joined);
    }
}

// ============================================================================
// Main entry point
// ============================================================================
int WINAPI WinMain(HINSTANCE hInstance, HINSTANCE, LPSTR lpCmdLine, int nCmdShow) {
    // Parse command line
    CliArgs cliArgs = ParseCommandLine(lpCmdLine);

    // In CLI mode, ensure stdout is unbuffered
    if (cliArgs.cliMode) {
        setvbuf(stdout, nullptr, _IONBF, 0);
        setvbuf(stderr, nullptr, _IONBF, 0);
    }

    // Initialize unified logger (fresh ipmsg_gui_debug.log, redirect cout/cerr).
    // The data directory is UTF-8 (custom dir from the registry or
    // %USERPROFILE%\.speedipmsg, with a port suffix for non-default ports).
    std::string dataDir = ipmsg::paths::ResolveDataDir(cliArgs.port);
    ipmsg::SetLogLevel(cliArgs.verbose ? ipmsg::LogLevel::Debug : ipmsg::LogLevel::Info);
    ipmsg::InitLogger(dataDir);

    // Install global crash handlers so hard faults (access violation / heap
    // corruption) and uncaught C++ exceptions are written to the log instead of
    // failing silently. The log is the single source of truth for crash analysis.
    std::set_terminate([]() {
        ipmsg::LogMessage("CRASH", "", "std::terminate called (uncaught C++ exception) - aborting");
        abort();
    });
    SetUnhandledExceptionFilter([](EXCEPTION_POINTERS* ep) -> LONG {
        auto toHex = [](uint64_t v) {
            char buf[24] = {};
            snprintf(buf, sizeof(buf), "%llX", static_cast<unsigned long long>(v));
            return std::string(buf);
        };
        std::string code = "0x" + toHex(ep->ExceptionRecord->ExceptionCode);
        std::string addr = "0x" + toHex(reinterpret_cast<uintptr_t>(ep->ExceptionRecord->ExceptionAddress));
        ipmsg::LogMessage("CRASH", "", "Unhandled SEH exception code=" + code + " at address=" + addr);
        return EXCEPTION_CONTINUE_SEARCH;
    });

    LOG_INFO("========================================");
    LOG_INFO("IPMsgPro starting...");
    LOG_INFO("Unified log: " + dataDir + "\\ipmsg_gui_debug.log");
    LOG_INFO("Port: " + std::to_string(cliArgs.port));
    LOG_INFO(std::string("Log level: ") + (cliArgs.verbose ? "DEBUG (--verbose)" : "INFO"));
    if (cliArgs.cliMode) {
        LOG_INFO("Mode: CLI (" + cliArgs.subCmd + ")");
    }

    // Initialize Winsock
    if (!ipmsg::WSAInit()) {
        LOG_ERROR("Failed to initialize Winsock");
        return 1;
    }
    LOG_INFO("Winsock initialized");

    // 输出系统版本、默认语言/地区及本机 IP
    LogSystemInfo();

    // Create core components
    g_msgMng = new ipmsg::MsgMng();
    g_msgDb = new ipmsg::MessageDB();
    g_fileTransfer = new ipmsg::FileTransferManager();
    LOG_INFO("Core components created");

    // Initialize message database (separate DB per port)
    std::string dbPath = dataDir + "\\ipmsg.db";
    if (!g_msgDb->Init(dbPath)) {
        LOG_ERROR("Failed to initialize database: " + dbPath);
    } else {
        LOG_INFO("Database initialized: " + dbPath);
    }

    // Initialize file transfer manager
    // Use MsgMng port for TCP file transfer (same port as Feiq protocol)
    // This ensures multiple instances on different UDP ports have different TCP ports
    g_fileTransfer->Init(cliArgs.port);
    LOG_INFO("File transfer manager initialized (TCP port=" +
             std::to_string(g_fileTransfer->GetTcpPort()) + ")");

    // ============================================================
    // CLI Mode (no GUI)
    // ============================================================
    if (cliArgs.cliMode) {
        // Initialize MsgMng with specified port
        if (!g_msgMng->Init(cliArgs.port)) {
            LOG_ERROR("Failed to initialize MsgMng on port " + std::to_string(cliArgs.port));
            delete g_fileTransfer;
            delete g_msgDb;
            delete g_msgMng;
            ipmsg::WSACleanup();
            return 1;
        }
        LOG_INFO("MsgMng initialized on port " + std::to_string(cliArgs.port));

        LOG_INFO("Local user: " + g_msgMng->GetLocalUser().userName +
                 "@" + g_msgMng->GetLocalUser().hostName + " (port=" +
                 std::to_string(cliArgs.port) + ")");

        int rc = ipmsg::cli::Run(cliArgs.subCmd, cliArgs.port, cliArgs.configPath,
                                 cliArgs.targetIp, cliArgs.targetPort,
                                 *g_msgMng, *g_fileTransfer);

        // Cleanup
        g_msgMng->Shutdown();
        delete g_fileTransfer;
        delete g_msgDb;
        delete g_msgMng;
        ipmsg::WSACleanup();
        ipmsg::ShutdownLogger();
        return rc;
    }

    // ============================================================
    // Normal GUI Mode
    // ============================================================

    // Configure application
    tauricpp::App::Config config;
    config.window_config.title = "迅秋";
    config.window_config.width = 960;
    config.window_config.height = 640;
    config.window_config.center = true;
    config.window_config.devtools = true;

    tauricpp::App app(config);
    LOG_INFO("TauriCPP app created");

    // Initialize command handler
    auto& cmdHandler = ipmsg::CommandHandler::Instance();
    cmdHandler.Init(app.GetBridge(), *g_msgMng, *g_msgDb, *g_fileTransfer);
    LOG_INFO("Command handler initialized");

    // Register all bridge commands
    cmdHandler.RegisterAllCommands();
    LOG_INFO("Bridge commands registered");

    // Setup app lifecycle
    static std::atomic<bool> g_running{true};

    app.OnSetup([&](tauricpp::App& app) {
        LOG_INFO("App setup callback started");

        // Set main window handle AFTER the native window is created
        // (OnSetup is called before Window::Run which creates the native window)
        app.GetWindow().OnCreated([&](tauricpp::Window& win) {
            LOG_INFO("Window created, hwnd=" + std::to_string(reinterpret_cast<uintptr_t>(win.GetHwnd())));
            cmdHandler.SetNativeWindowHandle(win.GetHwnd());
            cmdHandler.SetWindow(&win);

            // Create tray icon (loads from exe resources automatically)
            win.CreateTrayIcon("", "迅秋");

            // Tray left-click: show window
            win.OnTrayClick([&]() {
                win.Show();
            });

            // Tray context menu
            win.SetTrayMenu({
                {"显示主窗口", [&]() { win.Show(); }},
                {"退出", [&]() { g_running = false; win.Close(); }}
            });
        });

        // Set window close handler - intercept based on minimizeBehavior setting
        // minimizeBehavior: "taskbar" = minimize to taskbar, "tray" = hide to tray
        app.GetWindow().OnClose([&]() -> bool {
            // If g_running is already false, this is a real quit request (from tray menu)
            if (!g_running) return true;

            std::string behavior = cmdHandler.GetMinimizeBehavior();
            LOG_INFO("Window close requested, behavior=" + behavior);
            if (behavior == "tray") {
                // Hide window to tray instead of closing
                app.GetWindow().Hide();
                return false;  // Block close, just hide
            }
            // Default: actually close the window
            g_running = false;
            return true;
        });

        // Initialize MsgMng (network layer) with specified port
        if (!g_msgMng->Init(cliArgs.port)) {
            LOG_ERROR("Failed to initialize MsgMng on port " + std::to_string(cliArgs.port));
        } else {
            LOG_INFO("MsgMng initialized on port " + std::to_string(cliArgs.port));
            LOG_INFO("Local user: " + g_msgMng->GetLocalUser().userName +
                     "@" + g_msgMng->GetLocalUser().hostName);
        }

        // Setup event forwarding after MsgMng is initialized
        cmdHandler.SetupEventForwarding();
        LOG_INFO("Event forwarding setup complete");

        // Broadcast entry to discover users on the network
        // Skip broadcast if --adduser was specified (we already know the target)
        if (cliArgs.addUsers.empty()) {
            g_msgMng->BroadcastEntry();
            LOG_INFO("Broadcast entry sent");
        } else {
            LOG_INFO("Skipping broadcast (--adduser specified, using direct discovery)");
        }

        // Auto-add users specified via --adduser argument
        for (const auto& userEntry : cliArgs.addUsers) {
            size_t colonPos = userEntry.find(':');
            std::string ip = userEntry;
            int port = ipmsg::IPMSG_DEFAULT_PORT;
            if (colonPos != std::string::npos) {
                ip = userEntry.substr(0, colonPos);
                // A bad --adduser value must not abort startup before the window exists.
                try { port = std::stoi(userEntry.substr(colonPos + 1)); } catch (...) { port = 0; }
            }
            if (ip.empty() || port <= 0 || port > 65535) {
                LOG_WARN("Ignoring invalid --adduser entry: " + userEntry);
                continue;
            }
            LOG_INFO("Auto-adding user: " + ip + ":" + std::to_string(port));
            // Send BR_ENTRY directly to the target's listening port so both sides
            // can discover each other (different ports can't discover via broadcast alone)
            g_msgMng->SendDirectEntry(ip, port);
        }

        // Auto-add direct users from config (cross-subnet)
        for (const auto& [ip, port] : g_msgMng->GetDirectUsers()) {
            LOG_INFO("Auto-adding direct user from config: " + ip + ":" + std::to_string(port));
            g_msgMng->SendDirectEntry(ip, port);
        }

        // Auto-scan IP ranges from config
        auto scanRanges = g_msgMng->GetScanRanges();
        if (!scanRanges.empty()) {
            LOG_INFO("Auto-scanning " + std::to_string(scanRanges.size()) + " IP ranges from config");
            // Use default port and delay
            g_msgMng->ScanIpRanges(scanRanges, cliArgs.port, 50);
        }
    });

    LOG_INFO("Starting app event loop...");
    
    // Run the application
    int result = app.Run();

    LOG_INFO("App event loop ended, cleaning up...");

    // Stop image workers while their network/database dependencies still exist.
    cmdHandler.Shutdown();
    g_msgMng->Shutdown();
    ipmsg::WSACleanup();

    delete g_fileTransfer;
    delete g_msgDb;
    delete g_msgMng;

    LOG_INFO("Cleanup complete, exiting.");
    
    ipmsg::ShutdownLogger();

    return result;
}
