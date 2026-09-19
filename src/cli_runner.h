#pragma once
// ============================================================================
// Headless CLI mode (--mode=cli): protocol test harness, no window.
// ============================================================================

#include <string>

namespace ipmsg {
class MsgMng;
class FileTransferManager;

namespace cli {

/// Run "--cmd=server" (auto-accept files, echo text) or "--cmd=test" (send
/// items from a JSON config to --target). `msgMng` and `fileTransfer` must
/// already be initialized; they stay owned by the caller.
/// Returns the process exit code (0 on success, 2 for an unknown sub-command).
int Run(const std::string& subCmd, int port, const std::string& configPath,
        const std::string& targetIp, int targetPort,
        MsgMng& msgMng, FileTransferManager& fileTransfer);

}  // namespace cli
}  // namespace ipmsg
