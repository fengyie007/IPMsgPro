// ============================================================================
// Unified logger implementation
// ============================================================================
#include "logger.h"
#include "util/encoding.h"

#include <chrono>
#include <cstdio>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <mutex>
#include <sstream>
#include <streambuf>

namespace ipmsg {

namespace {

    std::ofstream g_log;
    std::mutex g_logMutex;
    std::string g_logPath;
    std::streambuf* g_oldCoutBuf = nullptr;
    std::streambuf* g_oldCerrBuf = nullptr;
    bool g_initialized = false;

    // Redirects std::cout / std::cerr into the log file while keeping each
    // line atomic: characters are buffered and flushed (under the log mutex)
    // on newline, so redirected stream output cannot interleave with the lines
    // produced by LogMessage().
    class LogStreamBuf : public std::streambuf {
    public:
        LogStreamBuf(std::streambuf* sink, std::mutex& mtx)
            : sink_(sink), mtx_(mtx) {
            setp(buf_, buf_ + sizeof(buf_));
        }

    protected:
        int_type overflow(int_type c) override {
            if (c == EOF) return sync();
            *pptr() = static_cast<char>(c);
            pbump(1);
            if (c == '\n') sync();
            return c;
        }

        int sync() override {
            std::lock_guard<std::mutex> lock(mtx_);
            if (pbase() != pptr()) {
                sink_->sputn(pbase(), static_cast<std::streamsize>(pptr() - pbase()));
                setp(buf_, buf_ + sizeof(buf_));
            }
            return sink_->pubsync();
        }

    private:
        std::streambuf* sink_;
        std::mutex& mtx_;
        char buf_[512];
    };

    LogStreamBuf* g_logBuf = nullptr;

    std::string GetTimestamp() {
        auto now = std::chrono::system_clock::now();
        auto time = std::chrono::system_clock::to_time_t(now);
        auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(
            now.time_since_epoch()) % 1000;
        std::stringstream ss;
        ss << std::put_time(std::localtime(&time), "%Y-%m-%d %H:%M:%S");
        ss << '.' << std::setfill('0') << std::setw(3) << ms.count();
        return ss.str();
    }

}  // namespace

void InitLogger(const std::string& dataDir) {
    if (g_initialized) return;
    g_initialized = true;

    g_logPath = dataDir + "\\ipmsg_gui_debug.log";

    // Fresh file every startup (truncate any previous content). The path is
    // UTF-8, so open through a wide std::filesystem::path.
    g_log.open(enc::PathFromUtf8(g_logPath), std::ios::trunc);
    if (!g_log.is_open()) {
        std::cerr << "Failed to open unified log file: " << g_logPath << std::endl;
        return;
    }

    // Redirect std::cout / std::cerr into the unified log (line-buffered,
    // mutex-guarded so output stays readable).
    g_logBuf = new LogStreamBuf(g_log.rdbuf(), g_logMutex);
    g_oldCoutBuf = std::cout.rdbuf(g_logBuf);
    g_oldCerrBuf = std::cerr.rdbuf(g_logBuf);
}

void ReinitLogger(const std::string& newDataDir) {
    std::lock_guard<std::mutex> lock(g_logMutex);

    // Close current log
    if (g_log.is_open()) {
        g_log.flush();
        g_log.close();
    }

    // Delete old stream buffer
    if (g_logBuf) {
        delete g_logBuf;
        g_logBuf = nullptr;
    }

    // Restore original streams
    if (g_oldCoutBuf) {
        std::cout.rdbuf(g_oldCoutBuf);
        g_oldCoutBuf = nullptr;
    }
    if (g_oldCerrBuf) {
        std::cerr.rdbuf(g_oldCerrBuf);
        g_oldCerrBuf = nullptr;
    }

    // Open new log at new location (append mode to preserve old logs)
    g_logPath = newDataDir + "\\ipmsg_gui_debug.log";
    g_log.open(enc::PathFromUtf8(g_logPath), std::ios::app);
    if (!g_log.is_open()) {
        std::cerr << "Failed to open unified log file at new location: " << g_logPath << std::endl;
        return;
    }

    // Create new stream buffer pointing to new log
    g_logBuf = new LogStreamBuf(g_log.rdbuf(), g_logMutex);
    g_oldCoutBuf = std::cout.rdbuf(g_logBuf);
    g_oldCerrBuf = std::cerr.rdbuf(g_logBuf);

    LogMessage("LOGGER", "", "[Logger] Reinitialized at new location: " + g_logPath);
}

void LogMessage(const std::string& tag, const std::string& level,
                const std::string& msg) {
    std::lock_guard<std::mutex> lock(g_logMutex);

    // Lazily open (append) if logging happened before InitLogger().
    if (!g_log.is_open() && !g_logPath.empty()) {
        g_log.open(enc::PathFromUtf8(g_logPath), std::ios::app);
    }
    if (!g_log.is_open()) return;

    std::string line = "[" + GetTimestamp() + "]";
    if (!tag.empty()) line += " [" + tag + "]";
    if (!level.empty()) line += " [" + level + "]";
    line += " " + msg + "\n";

    g_log << line;
    g_log.flush();
}

void ShutdownLogger() {
    if (!g_initialized) return;
    g_initialized = false;

    // Restore original streams before closing the file.
    if (g_oldCoutBuf) {
        std::cout.rdbuf(g_oldCoutBuf);
        g_oldCoutBuf = nullptr;
    }
    if (g_oldCerrBuf) {
        std::cerr.rdbuf(g_oldCerrBuf);
        g_oldCerrBuf = nullptr;
    }
    if (g_log.is_open()) g_log.close();
    delete g_logBuf;
    g_logBuf = nullptr;
}

}  // namespace ipmsg
