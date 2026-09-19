// ============================================================================
// Notification sound: the embedded notification.mp3 is extracted to the app
// temp directory once and played through MCI.
// ============================================================================
#include "bridge/notification_sound.h"
#include "logger.h"
#include "util/app_paths.h"
#include "util/encoding.h"
#include "../resources/resource.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <Windows.h>
#include <mmsystem.h>

#include <filesystem>
#include <fstream>
#include <string>
#include <thread>

namespace fs = std::filesystem;

namespace ipmsg {

using enc::Utf8ToWide;

namespace {


// Extract embedded notification.mp3 resource to a temp file (once) and return its path
std::string GetNotificationSoundPath() {
    // %TEMP%\IPMsgPro\notification.mp3 (UTF-8; MCI needs the ANSI form)
    std::string outPath = paths::AppTempDir() + "\\notification.mp3";
    const fs::path outFsPath = enc::PathFromUtf8(outPath);

    // If already extracted, reuse it
    {
        std::error_code ec;
        if (fs::exists(outFsPath, ec)) return outPath;
    }

    // Extract from embedded resource
    HMODULE hModule = GetModuleHandle(nullptr);
    HRSRC hRes = FindResourceA(hModule, MAKEINTRESOURCEA(IDR_NOTIFICATION_MP3), RT_RCDATA);
    if (!hRes) {
        LogMessage("BRIDGE", "ERROR", "[SOUND] Failed to find notification.mp3 resource");
        return "";
    }
    HGLOBAL hGlobal = LoadResource(hModule, hRes);
    if (!hGlobal) return "";
    DWORD size = SizeofResource(hModule, hRes);
    void* pData = LockResource(hGlobal);
    if (!pData || size == 0) return "";

    std::ofstream out(outFsPath, std::ios::binary);
    if (!out.good()) return "";
    out.write(reinterpret_cast<const char*>(pData), size);
    out.close();
    LogMessage("BRIDGE", "", "[SOUND] Extracted notification.mp3 to " + outPath);
    return outPath;
}

}  // namespace

// Play notification sound from embedded resource
void PlayNotificationSound() {
    std::string soundPath = GetNotificationSoundPath();
    if (soundPath.empty()) {
        LogMessage("BRIDGE", "WARN", "[SOUND] No sound path, aborting");
        return;
    }

    // Close any previous playback to avoid device conflicts
    mciSendStringW(L"close notify_snd", nullptr, 0, nullptr);

    // Use mciSendString to play MP3 asynchronously. The wide API is used so
    // a %TEMP% under a non-ASCII user name still resolves.
    std::wstring wPath = Utf8ToWide(soundPath);
    std::wstring openCmd = L"open \"" + wPath + L"\" type mpegvideo alias notify_snd";
    MCIERROR err = mciSendStringW(openCmd.c_str(), nullptr, 0, nullptr);
    if (err != 0) {
        // Fallback: try without explicit type
        std::wstring openCmd2 = L"open \"" + wPath + L"\" alias notify_snd";
        err = mciSendStringW(openCmd2.c_str(), nullptr, 0, nullptr);
    }
    if (err == 0) {
        mciSendStringW(L"play notify_snd from 0", nullptr, 0, nullptr);
        // Auto-close after a delay to release the device
        std::thread([]() {
            Sleep(3000);
            mciSendStringW(L"close notify_snd", nullptr, 0, nullptr);
        }).detach();
        LogMessage("BRIDGE", "DEBUG", "[SOUND] Playing notification sound");
    } else {
        LogMessage("BRIDGE", "WARN", "[SOUND] mciSendString open failed, err=" + std::to_string(err));
    }
}

}  // namespace ipmsg
