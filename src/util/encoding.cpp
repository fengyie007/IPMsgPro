// ============================================================================
// Text encoding helpers implementation
// ============================================================================
#include "util/encoding.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <Windows.h>

#include <vector>

namespace ipmsg {
namespace enc {

namespace {

bool IsAscii(const std::string& s) {
    for (unsigned char c : s) {
        if (c >= 0x80) return false;
    }
    return true;
}

std::wstring MultiByteToWide(UINT codePage, const std::string& in) {
    if (in.empty()) return {};
    int len = MultiByteToWideChar(codePage, 0, in.data(), static_cast<int>(in.size()), nullptr, 0);
    if (len <= 0) return {};
    std::wstring out(static_cast<size_t>(len), L'\0');
    MultiByteToWideChar(codePage, 0, in.data(), static_cast<int>(in.size()), &out[0], len);
    return out;
}

std::string WideToMultiByte(UINT codePage, const std::wstring& in) {
    if (in.empty()) return {};
    int len = WideCharToMultiByte(codePage, 0, in.data(), static_cast<int>(in.size()),
                                  nullptr, 0, nullptr, nullptr);
    if (len <= 0) return {};
    std::string out(static_cast<size_t>(len), '\0');
    WideCharToMultiByte(codePage, 0, in.data(), static_cast<int>(in.size()),
                        &out[0], len, nullptr, nullptr);
    return out;
}

}  // namespace

std::wstring Utf8ToWide(const std::string& utf8) {
    return MultiByteToWide(CP_UTF8, utf8);
}

std::string WideToUtf8(const std::wstring& wide) {
    return WideToMultiByte(CP_UTF8, wide);
}

std::string Utf8ToAnsi(const std::string& utf8) {
    if (IsAscii(utf8)) return utf8;
    std::wstring w = MultiByteToWide(CP_UTF8, utf8);
    if (w.empty()) return utf8;
    std::string out = WideToMultiByte(CP_ACP, w);
    return out.empty() ? utf8 : out;
}

std::string AnsiToUtf8(const std::string& ansi) {
    if (IsAscii(ansi)) return ansi;
    std::wstring w = MultiByteToWide(CP_ACP, ansi);
    if (w.empty()) return ansi;
    std::string out = WideToMultiByte(CP_UTF8, w);
    return out.empty() ? ansi : out;
}

bool IsValidUtf8(const std::string& s) {
    const unsigned char* p = reinterpret_cast<const unsigned char*>(s.data());
    const unsigned char* end = p + s.size();
    while (p < end) {
        if (*p < 0x80) { ++p; continue; }
        int extra;
        if ((*p & 0xE0) == 0xC0) extra = 1;
        else if ((*p & 0xF0) == 0xE0) extra = 2;
        else if ((*p & 0xF8) == 0xF0) extra = 3;
        else return false;  // stray continuation byte or invalid lead byte
        if (end - p < extra + 1) return false;
        for (int i = 1; i <= extra; ++i) {
            if ((p[i] & 0xC0) != 0x80) return false;
        }
        p += extra + 1;
    }
    return true;
}

std::string EnsureUtf8(const std::string& s) {
    if (s.empty() || IsValidUtf8(s)) return s;
    return AnsiToUtf8(s);
}

std::filesystem::path PathFromUtf8(const std::string& utf8) {
    return std::filesystem::path(Utf8ToWide(utf8));
}

std::string GetEnvUtf8(const wchar_t* name) {
    DWORD len = GetEnvironmentVariableW(name, nullptr, 0);
    if (len == 0) return {};
    std::vector<wchar_t> buf(len);
    DWORD got = GetEnvironmentVariableW(name, buf.data(), len);
    if (got == 0 || got >= len) return {};
    return WideToUtf8(std::wstring(buf.data(), got));
}

}  // namespace enc
}  // namespace ipmsg
