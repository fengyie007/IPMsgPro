// ============================================================================
// Unified logger
// ----------------------------------------------------------------------------
// All application logging (previously split across ipmsgpro.log, msgmng.log
// and ipmsg_gui_debug.log) is consolidated into a single file:
//
//     <dataDir>/ipmsg_gui_debug.log
//
// The file is recreated fresh (truncated) every time the application starts.
// std::cout and std::cerr are redirected into this same file so that any
// output written to the standard streams also lands in the unified log.
//
// Levels: DEBUG < INFO < WARN < ERROR. The default threshold is INFO; start
// the application with --verbose to also record DEBUG lines (per-packet hex
// dumps, per-chunk transfer progress, protocol field traces). Keep DEBUG
// output behind IsDebugEnabled() when building it is itself expensive.
// ============================================================================
#pragma once

#include <string>

namespace ipmsg {

enum class LogLevel : int {
    Debug = 0,
    Info  = 1,
    Warn  = 2,
    Error = 3,
};

// Set / query the minimum level that is written to the log. Thread-safe.
void SetLogLevel(LogLevel level);
LogLevel GetLogLevel();
inline bool IsDebugEnabled() { return GetLogLevel() <= LogLevel::Debug; }

// Initialize the unified logger. Opens <dataDir>/ipmsg_gui_debug.log in
// truncate mode (fresh each startup) and redirects std::cout / std::cerr
// into it. Safe to call once at process start.
void InitLogger(const std::string& dataDir);

// Reinitialize logger with a new data directory (closes old log, opens new one).
// Use when config-specified dataDir is loaded after initial startup.
void ReinitLogger(const std::string& newDataDir);

// Write a single tagged line to the unified log:
//   [YYYY-MM-DD HH:MM:SS.mmm] [TAG] [LEVEL] message
// `level` is "DEBUG", "INFO" (or "" for INFO), "WARN" or "ERROR"; lines below
// the current threshold are dropped. Thread-safe. If the logger has not been
// initialized yet, it lazily opens the file in append mode so early messages
// are not lost.
void LogMessage(const std::string& tag, const std::string& level,
                const std::string& msg);

// Flush and restore the original std::cout / std::cerr streams, then close
// the log file. Call once during shutdown.
void ShutdownLogger();

}  // namespace ipmsg
