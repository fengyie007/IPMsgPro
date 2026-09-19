// ============================================================================
// Application data paths implementation
// ============================================================================
#include "util/app_paths.h"
#include "util/encoding.h"
#include "ipmsg/protocol.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <Windows.h>
#include <ShlObj.h>

#include <vector>

namespace ipmsg {
namespace paths {

namespace {

constexpr const wchar_t* kRegKey = L"Software\\SpeedIPMsg";
constexpr const wchar_t* kRegDataDirValue = L"DataDir";

std::string KnownFolderUtf8(int csidl) {
    wchar_t buf[MAX_PATH] = {};
    if (SUCCEEDED(SHGetFolderPathW(nullptr, csidl, nullptr, 0, buf))) {
        return enc::WideToUtf8(buf);
    }
    return {};
}

std::string UserProfileDir() {
    std::string dir = enc::GetEnvUtf8(L"USERPROFILE");
    if (dir.empty()) dir = KnownFolderUtf8(CSIDL_PROFILE);
    return dir;
}

void EnsureDirectory(const std::string& utf8Dir) {
    if (utf8Dir.empty()) return;
    CreateDirectoryW(enc::Utf8ToWide(utf8Dir).c_str(), nullptr);
}

}  // namespace

std::string DefaultDataDir() {
    std::string base = UserProfileDir();
    if (base.empty()) base = KnownFolderUtf8(CSIDL_LOCAL_APPDATA);
    return base + "\\.speedipmsg";
}

std::string ReadCustomDataDir() {
    HKEY hKey = nullptr;
    if (RegOpenKeyExW(HKEY_CURRENT_USER, kRegKey, 0, KEY_READ, &hKey) != ERROR_SUCCESS) {
        return {};
    }
    std::string result;
    DWORD type = 0;
    DWORD size = 0;
    if (RegQueryValueExW(hKey, kRegDataDirValue, nullptr, &type, nullptr, &size) == ERROR_SUCCESS &&
        type == REG_SZ && size >= sizeof(wchar_t)) {
        std::vector<wchar_t> buf(size / sizeof(wchar_t) + 1, L'\0');
        if (RegQueryValueExW(hKey, kRegDataDirValue, nullptr, nullptr,
                             reinterpret_cast<LPBYTE>(buf.data()), &size) == ERROR_SUCCESS) {
            result = enc::WideToUtf8(std::wstring(buf.data()));  // stops at the NUL
        }
    }
    RegCloseKey(hKey);
    return result;
}

bool WriteCustomDataDir(const std::string& utf8Dir) {
    HKEY hKey = nullptr;
    if (RegCreateKeyExW(HKEY_CURRENT_USER, kRegKey, 0, nullptr, 0, KEY_WRITE,
                        nullptr, &hKey, nullptr) != ERROR_SUCCESS) {
        return false;
    }
    LSTATUS st;
    if (utf8Dir.empty()) {
        st = RegDeleteValueW(hKey, kRegDataDirValue);
        if (st == ERROR_FILE_NOT_FOUND) st = ERROR_SUCCESS;
    } else {
        std::wstring w = enc::Utf8ToWide(utf8Dir);
        st = RegSetValueExW(hKey, kRegDataDirValue, 0, REG_SZ,
                            reinterpret_cast<const BYTE*>(w.c_str()),
                            static_cast<DWORD>((w.size() + 1) * sizeof(wchar_t)));
    }
    RegCloseKey(hKey);
    return st == ERROR_SUCCESS;
}

std::string ApplyPortSuffix(const std::string& dir, int port) {
    if (port == IPMSG_DEFAULT_PORT) return dir;
    return dir + "_" + std::to_string(port);
}

std::string ResolveDataDir(int port) {
    std::string dir = ReadCustomDataDir();
    if (dir.empty()) dir = DefaultDataDir();
    dir = ApplyPortSuffix(dir, port);
    EnsureDirectory(dir);
    return dir;
}

std::string UserDownloadsDir() {
    std::string base = UserProfileDir();
    if (base.empty()) return "Downloads";
    return base + "\\Downloads";
}

std::string AppTempDir() {
    wchar_t tmp[MAX_PATH] = {};
    DWORD n = GetTempPathW(MAX_PATH, tmp);
    std::string dir = (n > 0 && n < MAX_PATH) ? enc::WideToUtf8(std::wstring(tmp, n)) : std::string(".\\");
    if (!dir.empty() && dir.back() != '\\') dir += '\\';
    dir += "IPMsgPro";
    EnsureDirectory(dir);
    return dir;
}

}  // namespace paths
}  // namespace ipmsg
