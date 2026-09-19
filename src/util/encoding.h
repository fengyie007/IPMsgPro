#pragma once
// ============================================================================
// Text encoding helpers (single definition for the whole backend)
// ----------------------------------------------------------------------------
// Conventions:
//   * Every std::string held in memory is UTF-8.
//   * Win32 file/registry/environment APIs are always called through their *W
//     variants; convert with Utf8ToWide / WideToUtf8 at the boundary.
//   * IPMsg/FeiQ wire payloads without IPMSG_UTF8OPT are in the system ANSI
//     code page (GBK on Chinese Windows); convert with Utf8ToAnsi / AnsiToUtf8.
// This header deliberately does not include <Windows.h>.
// ============================================================================

#include <filesystem>
#include <string>

namespace ipmsg {
namespace enc {

/// UTF-8 -> UTF-16. Invalid byte sequences become U+FFFD.
std::wstring Utf8ToWide(const std::string& utf8);

/// UTF-16 -> UTF-8.
std::string WideToUtf8(const std::wstring& wide);

/// UTF-8 -> system ANSI code page (GBK on zh-CN). ASCII passes through.
std::string Utf8ToAnsi(const std::string& utf8);

/// System ANSI code page -> UTF-8. ASCII passes through.
std::string AnsiToUtf8(const std::string& ansi);

/// Structural UTF-8 validity check (no NUL/overlong analysis, just sequences).
bool IsValidUtf8(const std::string& s);

/// Return `s` unchanged when it is valid UTF-8, otherwise treat it as ANSI
/// and convert. Idempotent, safe to call on already-converted text.
std::string EnsureUtf8(const std::string& s);

/// std::filesystem::path built from a UTF-8 string via UTF-16, so the path
/// reaches the *W file APIs intact (fs::path(std::string) would use ANSI).
std::filesystem::path PathFromUtf8(const std::string& utf8);

/// Read an environment variable as UTF-8. Empty when unset.
std::string GetEnvUtf8(const wchar_t* name);

}  // namespace enc
}  // namespace ipmsg
