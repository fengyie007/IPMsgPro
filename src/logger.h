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
// ============================================================================
#pragma once

#include <string>

namespace ipmsg {

// Initialize the unified logger. Opens <dataDir>/ipmsg_gui_debug.log in
// truncate mode (fresh each startup) and redirects std::cout / std::cerr
// into it. Safe to call once at process start.
void InitLogger(const std::string& dataDir);

// Write a single tagged line to the unified log:
//   [YYYY-MM-DD HH:MM:SS.mmm] [TAG] [LEVEL] message
// Thread-safe. If the logger has not been initialized yet, it lazily opens
// the file in append mode so early messages are not lost.
void LogMessage(const std::string& tag, const std::string& level,
                const std::string& msg);

// Flush and restore the original std::cout / std::cerr streams, then close
// the log file. Call once during shutdown.
void ShutdownLogger();

}  // namespace ipmsg
