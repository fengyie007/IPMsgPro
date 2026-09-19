#pragma once
// ============================================================================
// Application data paths (single definition for the whole backend)
// ----------------------------------------------------------------------------
// All returned paths are UTF-8 with backslashes. The custom data directory is
// persisted in the registry (HKCU\Software\SpeedIPMsg\DataDir) so it is known
// at process start, before the frontend has loaded its IndexedDB config.
// ============================================================================

#include <string>

namespace ipmsg {
namespace paths {

/// %USERPROFILE%\.speedipmsg (falls back to %LOCALAPPDATA% when USERPROFILE
/// is unset). No port suffix, directory is not created.
std::string DefaultDataDir();

/// Custom data directory from the registry, empty when not configured.
std::string ReadCustomDataDir();

/// Persist (non-empty) or clear (empty) the custom data directory.
bool WriteCustomDataDir(const std::string& utf8Dir);

/// Append "_<port>" when `port` is not the IPMsg default, so several
/// instances on different ports keep separate databases and logs.
std::string ApplyPortSuffix(const std::string& dir, int port);

/// Effective data directory: custom (if set) or default, with the port
/// suffix applied. The directory is created.
std::string ResolveDataDir(int port);

/// %USERPROFILE%\Downloads.
std::string UserDownloadsDir();

/// %TEMP%\IPMsgPro, created on first use.
std::string AppTempDir();

}  // namespace paths
}  // namespace ipmsg
