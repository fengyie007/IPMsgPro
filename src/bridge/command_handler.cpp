// ============================================================================
// Bridge Command Handler Implementation
// ============================================================================

// Prevent windows.h from including winsock.h (which conflicts with winsock2.h)
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <shellapi.h>
#include <mmsystem.h>

#include "command_handler.h"
#include "ipmsg/protocol.h"
#include "logger.h"
#include "../resources/resource.h"
#include <ctime>
#include <random>
#include <sstream>
#include <fstream>
#include <iomanip>
#include <cctype>
#include <cstring>
#include <cstdint>
#include <vector>
#include <tauricpp/dialog.hpp>
#include <shlobj.h>
#include <thread>
#include <filesystem>
#include <gdiplus.h>
#include <objidl.h>
#include <commdlg.h>
#include <unordered_map>

namespace fs = std::filesystem;
using namespace Gdiplus;

// ============================================================================
// Base64 encode/decode helpers (used by screenshot.capture and file.save_data)
// ============================================================================
static std::string Base64Encode(const std::string& in) {
    static const char* tbl =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    std::string out;
    out.reserve(((in.size() + 2) / 3) * 4);
    size_t i = 0;
    while (i + 3 <= in.size()) {
        uint32_t n = ((uint32_t)(unsigned char)in[i] << 16) |
                     ((uint32_t)(unsigned char)in[i + 1] << 8) |
                     (uint32_t)(unsigned char)in[i + 2];
        out.push_back(tbl[(n >> 18) & 0x3F]);
        out.push_back(tbl[(n >> 12) & 0x3F]);
        out.push_back(tbl[(n >> 6) & 0x3F]);
        out.push_back(tbl[n & 0x3F]);
        i += 3;
    }
    size_t rem = in.size() - i;
    if (rem == 1) {
        uint32_t n = (uint32_t)(unsigned char)in[i] << 16;
        out.push_back(tbl[(n >> 18) & 0x3F]);
        out.push_back(tbl[(n >> 12) & 0x3F]);
        out.push_back('=');
        out.push_back('=');
    } else if (rem == 2) {
        uint32_t n = ((uint32_t)(unsigned char)in[i] << 16) |
                     ((uint32_t)(unsigned char)in[i + 1] << 8);
        out.push_back(tbl[(n >> 18) & 0x3F]);
        out.push_back(tbl[(n >> 12) & 0x3F]);
        out.push_back(tbl[(n >> 6) & 0x3F]);
        out.push_back('=');
    }
    return out;
}

// Base64 decoder (inverse of Base64Encode). Used by file.save_data.
static std::string Base64Decode(const std::string& in) {
    static const std::string tbl =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
        "abcdefghijklmnopqrstuvwxyz"
        "0123456789+/";
    std::string out;
    int i = 0, j = 0;
    int in_len = (int)in.size();
    char c4[4], c3[3];
    while (in_len-- && in[i] != '=' && (isalnum((unsigned char)in[i]) || in[i] == '+' || in[i] == '/')) {
        c4[j++] = in[i++];
        if (j == 4) {
            for (j = 0; j < 4; j++) c4[j] = (char)tbl.find(c4[j]);
            c3[0] = (char)((c4[0] << 2) + ((c4[1] & 0x30) >> 4));
            c3[1] = (char)(((c4[1] & 0xf) << 4) + ((c4[2] & 0x3c) >> 2));
            c3[2] = (char)(((c4[2] & 0x3) << 6) + c4[3]);
            for (j = 0; j < 3; j++) out += c3[j];
            j = 0;
        }
    }
    if (j) {
        for (int k = j; k < 4; k++) c4[k] = 0;
        for (int k = 0; k < 4; k++) c4[k] = (char)tbl.find(c4[k]);
        c3[0] = (char)((c4[0] << 2) + ((c4[1] & 0x30) >> 4));
        c3[1] = (char)(((c4[1] & 0xf) << 4) + ((c4[2] & 0x3c) >> 2));
        c3[2] = (char)(((c4[2] & 0x3) << 6) + c4[3]);
        for (int k = 0; k < j - 1; k++) out += c3[k];
    }
    return out;
}

// CRC32 (IEEE 802.3) for diagnostics / DIB integrity checks.
static uint32_t Crc32(const std::string& data) {
    uint32_t crc = 0xFFFFFFFF;
    for (unsigned char c : data) {
        crc ^= c;
        for (int i = 0; i < 8; ++i)
            crc = (crc & 1) ? (0xEDB88320 ^ (crc >> 1)) : (crc >> 1);
    }
    return crc ^ 0xFFFFFFFF;
}

// ============================================================================
// Utility: Convert GBK to UTF-8 on Windows
// ============================================================================
static std::string GbkToUtf8(const std::string& gbk) {
    if (gbk.empty()) return gbk;
    // Check if already valid UTF-8 (quick heuristic: no bytes >= 0x80 means ASCII)
    bool isAscii = true;
    for (unsigned char c : gbk) {
        if (c >= 0x80) { isAscii = false; break; }
    }
    if (isAscii) return gbk;

    // GBK -> UTF-16
    int wlen = MultiByteToWideChar(CP_ACP, 0, gbk.c_str(), -1, nullptr, 0);
    if (wlen <= 0) return gbk;
    std::wstring wstr(wlen, L'\0');
    MultiByteToWideChar(CP_ACP, 0, gbk.c_str(), -1, &wstr[0], wlen);

    // UTF-16 -> UTF-8
    int ulen = WideCharToMultiByte(CP_UTF8, 0, wstr.c_str(), -1, nullptr, 0, nullptr, nullptr);
    if (ulen <= 0) return gbk;
    std::string utf8(ulen, '\0');
    WideCharToMultiByte(CP_UTF8, 0, wstr.c_str(), -1, &utf8[0], ulen, nullptr, nullptr);
    if (!utf8.empty() && utf8.back() == '\0') utf8.pop_back();

    return utf8;
}

// UTF-8 -> GBK (本地代码页)。用于把内部 UTF-8 文件名转换为 IPMsg/FeiQ 协议层所需的
// ANSI/GBK 字节，否则对方的飞秋/原生 UI 会把 UTF-8 字节当成 GBK 解码产生乱码。
static std::string Utf8ToGbk(const std::string& utf8) {
    if (utf8.empty()) return utf8;
    bool isAscii = true;
    for (unsigned char c : utf8) {
        if (c >= 0x80) { isAscii = false; break; }
    }
    if (isAscii) return utf8;

    // UTF-8 -> UTF-16
    int wlen = MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), -1, nullptr, 0);
    if (wlen <= 0) return utf8;
    std::wstring wstr(wlen, L'\0');
    MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), -1, &wstr[0], wlen);

    // UTF-16 -> GBK
    int glen = WideCharToMultiByte(CP_ACP, 0, wstr.c_str(), -1, nullptr, 0, nullptr, nullptr);
    if (glen <= 0) return utf8;
    std::string gbk(glen, '\0');
    WideCharToMultiByte(CP_ACP, 0, wstr.c_str(), -1, &gbk[0], glen, nullptr, nullptr);
    if (!gbk.empty() && gbk.back() == '\0') gbk.pop_back();

    return gbk;
}

// UTF-8 -> UTF-16 (wide string) for Win32 APIs.
static std::wstring Utf8ToWide(const std::string& utf8) {
    if (utf8.empty()) return {};
    int wlen = MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), -1, nullptr, 0);
    if (wlen <= 0) return {};
    std::wstring wstr(wlen, L'\0');
    MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), -1, &wstr[0], wlen);
    if (!wstr.empty() && wstr.back() == L'\0') wstr.pop_back();
    return wstr;
}

static bool IsValidUtf8(const std::string& str) {
    // Quick check: if all bytes are ASCII, it's valid UTF-8
    bool hasNonAscii = false;
    for (unsigned char c : str) {
        if (c >= 0x80) { hasNonAscii = true; break; }
    }
    if (!hasNonAscii) return true;

    // Full validation
    const unsigned char* p = reinterpret_cast<const unsigned char*>(str.data());
    const unsigned char* end = p + str.size();
    while (p < end) {
        if (*p < 0x80) { ++p; continue; }
        else if (*p < 0xC0) return false; // Unexpected continuation byte
        else if (*p < 0xE0) { if (end - p < 2 || (p[1] & 0xC0) != 0x80) return false; p += 2; }
        else if (*p < 0xF0) { if (end - p < 3 || (p[1] & 0xC0) != 0x80 || (p[2] & 0xC0) != 0x80) return false; p += 3; }
        else if (*p < 0xF8) { if (end - p < 4 || (p[1] & 0xC0) != 0x80 || (p[2] & 0xC0) != 0x80 || (p[3] & 0xC0) != 0x80) return false; p += 4; }
        else return false;
    }
    return true;
}

static std::string EnsureUtf8(const std::string& str) {
    if (IsValidUtf8(str)) return str;
    return GbkToUtf8(str);
}
#include <iostream>
#include <chrono>

// windows.h, shellapi.h, winsock2.h already included at top (with WIN32_LEAN_AND_MEAN)

namespace ipmsg {

namespace {

    // Extract embedded notification.mp3 resource to a temp file (once) and return its path
    std::string GetNotificationSoundPath() {
        // Temp file path: %TEMP%/IPMsgPro/notification.mp3
        char tempPath[MAX_PATH] = {};
        GetTempPathA(MAX_PATH, tempPath);
        std::string dir = std::string(tempPath) + "IPMsgPro";
        CreateDirectoryA(dir.c_str(), nullptr);
        std::string outPath = dir + "\\notification.mp3";

        // If already extracted, reuse it
        {
            std::ifstream f(outPath, std::ios::binary);
            if (f.good()) return outPath;
        }

        // Extract from embedded resource
        HMODULE hModule = GetModuleHandle(nullptr);
        HRSRC hRes = FindResourceA(hModule, MAKEINTRESOURCEA(IDR_NOTIFICATION_MP3), RT_RCDATA);
        if (!hRes) {
            LogMessage("BRIDGE", "", "[SOUND] Failed to find notification.mp3 resource");
            return "";
        }
        HGLOBAL hGlobal = LoadResource(hModule, hRes);
        if (!hGlobal) return "";
        DWORD size = SizeofResource(hModule, hRes);
        void* pData = LockResource(hGlobal);
        if (!pData || size == 0) return "";

        std::ofstream out(outPath, std::ios::binary);
        if (!out.good()) return "";
        out.write(reinterpret_cast<const char*>(pData), size);
        out.close();
        LogMessage("BRIDGE", "", "[SOUND] Extracted notification.mp3 to " + outPath);
        return outPath;
    }

    // Play notification sound from embedded resource
    void PlayNotificationSound() {
        std::string soundPath = GetNotificationSoundPath();
        if (soundPath.empty()) {
            LogMessage("BRIDGE", "", "[SOUND] No sound path, aborting");
            return;
        }

        // Close any previous playback to avoid device conflicts
        mciSendStringA("close notify_snd", nullptr, 0, nullptr);

        // Use mciSendString to play MP3 asynchronously
        std::string openCmd = "open \"" + soundPath + "\" type mpegvideo alias notify_snd";
        MCIERROR err = mciSendStringA(openCmd.c_str(), nullptr, 0, nullptr);
        if (err != 0) {
            // Fallback: try without explicit type
            std::string openCmd2 = "open \"" + soundPath + "\" alias notify_snd";
            err = mciSendStringA(openCmd2.c_str(), nullptr, 0, nullptr);
        }
        if (err == 0) {
            mciSendStringA("play notify_snd from 0", nullptr, 0, nullptr);
            // Auto-close after a delay to release the device
            std::thread([soundPath]() {
                Sleep(3000);
                mciSendStringA("close notify_snd", nullptr, 0, nullptr);
            }).detach();
            LogMessage("BRIDGE", "", "[SOUND] Playing notification sound");
        } else {
            LogMessage("BRIDGE", "", "[SOUND] mciSendString open failed, err=" + std::to_string(err));
        }
    }
}

CommandHandler& CommandHandler::Instance() {
    static CommandHandler instance;
    return instance;
}

void CommandHandler::Init(tauricpp::Bridge& bridge, MsgMng& msgMng,
                           MessageDB& msgDb, FileTransferManager& fileTransfer) {
    bridge_ = &bridge;
    msgMng_ = &msgMng;
    msgDb_ = &msgDb;
    fileTransfer_ = &fileTransfer;
}

void CommandHandler::SetNativeWindowHandle(void* hwnd) {
    hwnd_ = static_cast<void*>(hwnd);
    LogMessage("BRIDGE", "", "[DIALOG] SetNativeWindowHandle called, hwnd=" + 
                  (hwnd ? std::to_string(reinterpret_cast<uintptr_t>(hwnd)) : "NULL"));
}

void CommandHandler::SetWindow(tauricpp::Window* window) {
    window_ = window;
    LogMessage("BRIDGE", "", "[WINDOW] SetWindow called");
}

// ============================================================================
// FeiQ inline screenshot SEND channel
// ============================================================================
//
// IMPORTANT (verified against a real FeiQ capture): modern FeiQ sends inline
// screenshots as RAW JPEG bytes. The fragment payload is the JPEG file directly
// (magic 0xFFD8FFE0), NOT the legacy "LZW!" + DIB wrapper. The fragment header
// layout is:
//     "<id>|<total>|<off>|<fragCount>|<fragIndex>|<len>|0|2|0|<mtime>#"
// followed IMMEDIATELY by the raw JPEG chunk (NO leading 0x00). The reference
// message is a RICH-TEXT message whose body embeds the inline-image marker
// "/~#><id>" inside FeiQ's font/object placeholder "B~{/font;...}".

// Resolve a GDI+ image encoder CLSID by its MIME type.
static bool GetEncoderClsid(const wchar_t* mime, CLSID& clsid) {
    UINT num = 0, size = 0;
    Gdiplus::GetImageEncodersSize(&num, &size);
    if (size == 0) return false;
    auto* buf = (Gdiplus::ImageCodecInfo*)malloc(size);
    if (!buf) return false;
    Gdiplus::GetImageEncoders(num, size, buf);
    bool found = false;
    for (UINT i = 0; i < num; ++i) {
        if (wcscmp(buf[i].MimeType, mime) == 0) { clsid = buf[i].Clsid; found = true; break; }
    }
    free(buf);
    return found;
}

// GDI+ is initialized lazily and kept alive for the process lifetime. The
// screenshot-capture path starts/stops GDI+ locally, so Bitmap::FromStream in
// this file would otherwise run with GDI+ uninitialized and fail silently.
static bool EnsureGdiplus() {
    static ULONG_PTR token = 0;
    static int started = 0;
    if (!started) {
        Gdiplus::GdiplusStartupInput input;
        if (Gdiplus::GdiplusStartup(&token, &input, NULL) == Gdiplus::Ok) started = 1;
    }
    return started != 0;
}

static uint32_t ReadLE32(const std::string& s, size_t o) {
    if (o + 4 > s.size()) return 0;
    return (uint32_t)(unsigned char)s[o]
         | ((uint32_t)(unsigned char)s[o + 1] << 8)
         | ((uint32_t)(unsigned char)s[o + 2] << 16)
         | ((uint32_t)(unsigned char)s[o + 3] << 24);
}

// Decode a base64 PNG data URL (from the frontend screenshot editor) into a
// 24-bit bottom-up DIB (BITMAPINFOHEADER + pixel data, NO file header). FeiQ
// renders inline screenshots as an LZW-compressed 24/32-bit DIB, so we must
// hand it a DIB, not a JPEG.
static bool PngDataUrlTo24bppDib(const std::string& dataUrl, std::string& dib) {
    dib.clear();
    if (!EnsureGdiplus()) return false;
    std::string b64;
    auto comma = dataUrl.find(',');
    if (comma != std::string::npos) b64 = dataUrl.substr(comma + 1);
    else b64 = dataUrl;
    std::string png = Base64Decode(b64);
    if (png.empty()) return false;

    HGLOBAL hG = GlobalAlloc(GMEM_MOVEABLE, png.size());
    if (!hG) return false;
    memcpy(GlobalLock(hG), png.data(), png.size());
    GlobalUnlock(hG);

    IStream* pStream = nullptr;
    if (FAILED(CreateStreamOnHGlobal(hG, TRUE, &pStream))) { GlobalFree(hG); return false; }

    Gdiplus::Bitmap* bmp = Gdiplus::Bitmap::FromStream(pStream);
    bool ok = false;
    if (bmp && bmp->GetLastStatus() == Gdiplus::Ok) {
        HBITMAP hBmp = NULL;
        if (bmp->GetHBITMAP(Gdiplus::Color(0, 0, 0), &hBmp) == Gdiplus::Ok && hBmp) {
            BITMAP bm;
            if (GetObject(hBmp, sizeof(bm), &bm) == sizeof(bm)) {
                int w = bm.bmWidth, h = bm.bmHeight;
                if (w > 0 && h > 0) {
                    BITMAPINFOHEADER bi = { 0 };
                    bi.biSize = 40;
                    bi.biWidth = w;
                    bi.biHeight = h;          // positive => bottom-up DIB
                    bi.biPlanes = 1;
                    bi.biBitCount = 24;
                    bi.biCompression = 0;
                    int rowBytes = ((w * 3 + 3) / 4) * 4;
                    bi.biSizeImage = (uint32_t)(rowBytes * h);
                    HDC hdc = GetDC(NULL);
                    if (hdc) {
                    dib.resize(40 + bi.biSizeImage);
                    if (GetDIBits(hdc, hBmp, 0, h, (void*)(dib.data() + 40),
                                  (BITMAPINFO*)&bi, DIB_RGB_COLORS)) {
                        // GetDIBits writes the pixels into dib+40 but updates the
                        // BITMAPINFOHEADER in `bi`; the DIB FeiQ reads starts with a
                        // valid 40-byte header, so copy it to the front of the buffer.
                        memcpy((void*)dib.data(), &bi, 40);
                        ok = true;
                    }
                        ReleaseDC(NULL, hdc);
                    }
                }
            }
            DeleteObject(hBmp);
        }
    }
    delete bmp;
    if (pStream) pStream->Release();
    GlobalFree(hG);
    return ok;

}

// Read width/height from a JPEG's SOF marker (best-effort; returns false if it
// cannot be determined). Used to fill FeiQ's inline-image placeholder sizing.
static bool JpegDimensions(const std::string& jpeg, int& w, int& h) {
    w = 0; h = 0;
    if (jpeg.size() < 4 || (uint8_t)jpeg[0] != 0xFF || (uint8_t)jpeg[1] != 0xD8) return false;
    size_t i = 2;
    while (i + 9 < jpeg.size()) {
        if ((uint8_t)jpeg[i] != 0xFF) { i++; continue; }
        uint8_t marker = (uint8_t)jpeg[i + 1];
        // SOF markers (exclude DHT=0xC4, DAC=0xC8, lossless variants we don't need).
        if (marker >= 0xC0 && marker <= 0xCF &&
            marker != 0xC4 && marker != 0xC8 && marker != 0xCC) {
            h = ((uint8_t)jpeg[i + 5] << 8) | (uint8_t)jpeg[i + 6];
            w = ((uint8_t)jpeg[i + 7] << 8) | (uint8_t)jpeg[i + 8];
            return w > 0 && h > 0;
        }
        int len = ((uint8_t)jpeg[i + 2] << 8) | (uint8_t)jpeg[i + 3];
        if (len <= 0) break;
        i += 2 + (size_t)len;
    }
    return false;
}

// Forward declarations (defined later, after LzwDecompress).
static bool LzwCompress(const std::string& in, std::string& out);
static bool LzwDecompress(const std::string& in, size_t inOff, size_t inLen,
                          int minCodeSize, size_t expectedOut, std::string& out);

nlohmann::json CommandHandler::HandleFeiQScreenshotSend(const nlohmann::json& args) {
    nlohmann::json r; r["success"] = false;
    try {
        std::string targetArg = args.contains("target") && args["target"].is_string()
            ? args["target"].get<std::string>() : "<none>";
        LogMessage("BRIDGE", "", "[FEIQ-SHOT-TX] ENTER target=\"" + targetArg + "\" hasDataUrl=" +
            (args.contains("dataUrl") && args["dataUrl"].is_string() ? "yes" : "no"));
        auto target = FindUserFromArgs(args);
        if (!target) {
            LogMessage("BRIDGE", "", "[FEIQ-SHOT-TX] FIND TARGET FAILED for=\"" + targetArg + "\"");
            r["error"] = "未找到目标用户"; return r;
        }
        LogMessage("BRIDGE", "", "[FEIQ-SHOT-TX] target resolved key=" + target->Key() +
            " ip=" + target->ipAddress);
        if (!args.contains("dataUrl") || !args["dataUrl"].is_string()) {
            r["error"] = "缺少图片数据"; return r;
        }

        // FeiQ renders inline screenshots as an LZW-compressed DIB -- EXACTLY the
        // format it sends to us: "LZW!" + uint32 LE (DIB size) + uint32 LE (CRC32
        // of DIB) + LZW(DIB). We must emit this, NOT a raw JPEG.
        std::string dib;
        if (!PngDataUrlTo24bppDib(args["dataUrl"].get<std::string>(), dib)) {
            r["error"] = "图片编码失败"; return r;
        }
        int dw = (int)ReadLE32(dib, 4);
        int dh = (int)(int32_t)ReadLE32(dib, 8);
        LogMessage("BRIDGE", "", "[FEIQ-SHOT-TX] DIB bytes=" + std::to_string(dib.size()) +
            " w=" + std::to_string(dw) + " h=" + std::to_string(dh));

        uint32_t crc = Crc32(dib);
        std::string lzw;
        if (!LzwCompress(dib, lzw)) { r["error"] = "压缩失败"; return r; }

        // Self-check: our compressor MUST be decodable by the same LZW variant
        // FeiQ uses (our LzwDecompress mirrors it). If round-trip fails, the peer
        // could never decode our payload either -- abort and report instead of
        // sending a broken screenshot.
        {
            std::string round;
            if (!LzwDecompress(lzw, 0, lzw.size(), 8, dib.size(), round) || round != dib) {
                LogMessage("BRIDGE", "", "[FEIQ-SHOT-TX] SELFCHECK LZW round-trip MISMATCH (out=" +
                    std::to_string(round.size()) + " expected=" + std::to_string(dib.size()) + ")");
                r["error"] = "LZW 自检失败"; return r;
            }
            LogMessage("BRIDGE", "", "[FEIQ-SHOT-TX] SELFCHECK LZW round-trip OK (" +
                std::to_string(lzw.size()) + " bytes compressed)");
        }

        std::string payload = "LZW!";
        auto appendU32 = [&](uint32_t v) {
            payload.push_back((char)(v & 0xFF));
            payload.push_back((char)((v >> 8) & 0xFF));
            payload.push_back((char)((v >> 16) & 0xFF));
            payload.push_back((char)((v >> 24) & 0xFF));
        };
        appendU32((uint32_t)dib.size());
        appendU32(crc);
        payload += lzw;
        LogMessage("BRIDGE", "", "[FEIQ-SHOT-TX] payload=" + std::to_string(payload.size()) +
            " (LZW=" + std::to_string(lzw.size()) + ")");

        // FeiQ fragments the payload into 512-byte chunks (verified against a live
        // capture). Larger UDP datagrams get IP-fragmented and dropped, so FeiQ
        // never reassembles the image.
        // Diagnostics: dump the DIB we generated (for offline comparison with what
        // FeiQ itself sends) before emitting.
        {
            static unsigned dibSeq = 0;
            char db[16]; snprintf(db, sizeof(db), "%08X", ++dibSeq);
            std::ofstream o2(GetUserDownloadsDir() + "/FeiQ_OurTX_dib_" + std::string(db) + ".bin", std::ios::binary);
            if (o2) o2.write(dib.data(), (std::streamsize)dib.size());
        }

        // FeiQ sends the inline-image reference with command 0x00000121
        // (IPMSG_SENDMSG | IPMSG_SENDCHECKOPT | 0x1).
        const uint32_t kFeiQRefCmd = IPMSG_SENDMSG | IPMSG_SENDCHECKOPT | 0x1; // 0x121

        // Emit the inline screenshot to the peer using the fragmented FeiQ protocol.
        SendFeiQShotPayload(*target, payload, dw, dh, kFeiQRefCmd, IPMSG_FILEATTACHOPT | 0xC0);
        r["success"] = true;
        r["fragments"] = (int)((payload.size() + 511) / 512);
    } catch (const std::exception& e) {
        r["error"] = e.what();
    }
    return r;
}

nlohmann::json CommandHandler::HandleWindowSetAlwaysOnTop(const nlohmann::json& args) {
    nlohmann::json r; r["success"] = true;
    try {
        bool onTop = args.contains("on_top") ? args["on_top"].get<bool>() : true;
        if (window_) window_->SetAlwaysOnTop(onTop);
    } catch (const std::exception& e) {
        r["success"] = false; r["error"] = e.what();
    }
    return r;
}

void CommandHandler::RegisterAllCommands() {
    if (!bridge_) return;

    // User management
    bridge_->RegisterCommand("user.discover",
        [this](const nlohmann::json& args) { return HandleUserDiscover(args); });
    bridge_->RegisterCommand("user.list",
        [this](const nlohmann::json& args) { return HandleUserList(args); });
    bridge_->RegisterCommand("user.status",
        [this](const nlohmann::json& args) { return HandleUserStatus(args); });
    bridge_->RegisterCommand("user.local",
        [this](const nlohmann::json& args) { return HandleUserLocal(args); });

    // Message
    bridge_->RegisterCommand("message.send",
        [this](const nlohmann::json& args) { return HandleMessageSend(args); });
    bridge_->RegisterCommand("message.send_image",
        [this](const nlohmann::json& args) { return HandleMessageSendImage(args); });

    // File
    bridge_->RegisterCommand("file.send",
        [this](const nlohmann::json& args) { return HandleFileSend(args); });
    bridge_->RegisterCommand("file.info",
        [this](const nlohmann::json& args) { return HandleFileInfo(args); });
    bridge_->RegisterCommand("file.recv",
        [this](const nlohmann::json& args) { return HandleFileRecv(args); });
    bridge_->RegisterCommand("file.save_temp",
        [this](const nlohmann::json& args) { return HandleFileSaveTemp(args); });
    bridge_->RegisterCommand("file.accept",
        [this](const nlohmann::json& args) { return HandleFileAccept(args); });
    bridge_->RegisterCommand("file.reject",
        [this](const nlohmann::json& args) { return HandleFileReject(args); });
    bridge_->RegisterCommand("file.open_folder",
        [this](const nlohmann::json& args) { return HandleFileOpenFolder(args); });

    // History
    bridge_->RegisterCommand("history.get",
        [this](const nlohmann::json& args) { return HandleHistoryGet(args); });
    bridge_->RegisterCommand("history.search",
        [this](const nlohmann::json& args) { return HandleHistorySearch(args); });
    bridge_->RegisterCommand("history.clear",
        [this](const nlohmann::json& args) { return HandleHistoryClear(args); });

    // Network
    bridge_->RegisterCommand("network.scan",
        [this](const nlohmann::json& args) { return HandleNetworkScan(args); });

    // Config
    bridge_->RegisterCommand("config.set",
        [this](const nlohmann::json& args) { return HandleConfigSet(args); });

    // Dialog
    bridge_->RegisterCommand("dialog.pick_folder",
        [this](const nlohmann::json& args) { return HandleDialogPickFolder(args); });
    bridge_->RegisterCommand("dialog.open",
        [this](const nlohmann::json& args) { return HandleDialogOpen(args); });
    bridge_->RegisterCommand("shell_open",
        [this](const nlohmann::json& args) { return HandleShellOpen(args); });

    // --- Screenshot commands ---
    bridge_->RegisterCommand("screenshot.capture",
        [this](const nlohmann::json& args) { return HandleScreenshotCapture(args); });
    bridge_->RegisterCommand("window.maximize",
        [this](const nlohmann::json& args) { return HandleWindowMaximize(args); });
    bridge_->RegisterCommand("window.restore",
        [this](const nlohmann::json& args) { return HandleWindowRestore(args); });
    bridge_->RegisterCommand("window.set_always_on_top",
        [this](const nlohmann::json& args) { return HandleWindowSetAlwaysOnTop(args); });
    bridge_->RegisterCommand("feiq.screenshot_send",
        [this](const nlohmann::json& args) { return HandleFeiQScreenshotSend(args); });
    bridge_->RegisterCommand("feiq.echo_screenshot",
        [this](const nlohmann::json& args) { return HandleFeiQEchoScreenshot(args); });
    bridge_->RegisterCommand("dialog.save",
        [this](const nlohmann::json& args) { return HandleDialogSave(args); });
    bridge_->RegisterCommand("file.save_data",
        [this](const nlohmann::json& args) { return HandleFileSaveData(args); });

}

void CommandHandler::SetupEventForwarding() {
    if (!bridge_ || !msgMng_) return;

    // Setup file transfer progress callback
    fileTransfer_->SetProgressCallback([this](const ipmsg::TransferProgress& progress) {
        nlohmann::json event = {
            {"transferId", progress.transferId},
            {"filename", progress.filename},
            {"fileSize", progress.fileSize},
            {"transferred", progress.transferred},
            {"status", static_cast<int>(progress.status)},
            {"isSending", progress.isSending}
        };

        LogMessage("BRIDGE", "", "[PROGRESS-CB] transferId=" + progress.transferId +
                     ", status=" + std::to_string(static_cast<int>(progress.status)) +
                     ", transferred=" + std::to_string(progress.transferred) +
                     "/" + std::to_string(progress.fileSize) +
                     ", isSending=" + std::to_string(progress.isSending));

        try {
            if (progress.status == ipmsg::TransferStatus::Completed) {
                // File transfer completed
                event["message"] = progress.isSending ? "File sent successfully" : "File received successfully";
                if (!progress.isSending) {
                    event["savePath"] = progress.localPath;
                }
                LogMessage("BRIDGE", "", "[PROGRESS-CB] Emitting file.transfer_completed for transferId=" + progress.transferId);
                bridge_->Emit("file.transfer_completed", event);
            } else if (progress.status == ipmsg::TransferStatus::Failed) {
                // File transfer failed
                event["message"] = "File transfer failed";
                bridge_->Emit("file.transfer_failed", event);
            } else {
                // Progress update
                event["progress"] = progress.fileSize > 0 ?
                    (progress.transferred * 100.0 / progress.fileSize) : 0.0;
                bridge_->Emit("file.transfer_progress", event);
            }
        } catch (const std::exception& e) {
            LogMessage("BRIDGE", "", "[PROGRESS-CB] Emit threw for transferId=" + progress.transferId +
                          " filename=" + progress.filename + ": " + e.what());
        } catch (...) {
            LogMessage("BRIDGE", "", "[PROGRESS-CB] Emit threw unknown exception for transferId=" + progress.transferId);
        }
    });

    msgMng_->SetUserDiscoveredCallback([this](const ipmsg::UserInfo& user) {
        bridge_->Emit("user.discovered", UserToJson(user));
    });

    msgMng_->SetUserLeftCallback([this](const UserInfo& user) {
        bridge_->Emit("user.status_changed", {
            {"user", UserToJson(user)},
            {"status", "offline"}
        });
    });

    msgMng_->SetMessageReceivedCallback([this](const MsgBuf& msg) {
        try {
            // Log every received message for debugging
            {
                char cmdBuf[32] = {};
                snprintf(cmdBuf, sizeof(cmdBuf), "0x%08lx", (unsigned long)msg.command);
                uint32_t mode = GET_MODE(msg.command);
                std::string modeStr;
                switch (mode) {
                    case IPMSG_BR_ENTRY: modeStr = "BR_ENTRY"; break;
                    case IPMSG_BR_EXIT: modeStr = "BR_EXIT"; break;
                    case IPMSG_ANSENTRY: modeStr = "ANSENTRY"; break;
                    case IPMSG_SENDMSG: modeStr = "SENDMSG"; break;
                    case IPMSG_RECVMSG: modeStr = "RECVMSG"; break;
                    default: modeStr = "UNKNOWN"; break;
                }
                LogMessage("BRIDGE", "", "[GUI-MSG] ====== BEGIN MESSAGE ======");
                LogMessage("BRIDGE", "", "[GUI-MSG] packetNo=" + std::to_string(msg.packetNo));
                LogMessage("BRIDGE", "", std::string("[GUI-MSG] from=") + msg.sender.userName + "@" +
                              msg.sender.hostName + " (" + msg.sender.ipAddress + ":" +
                              std::to_string(msg.sender.portNo) + ")");
                LogMessage("BRIDGE", "", std::string("[GUI-MSG] nickName=") + msg.sender.nickName +
                              ", groupName=" + msg.sender.groupName);
                LogMessage("BRIDGE", "", std::string("[GUI-MSG] command=") + cmdBuf + " (" + modeStr + ")");
                {
                    std::string flags;
                    if (msg.command & IPMSG_SENDCHECKOPT) flags += "SENDCHECKOPT ";
                    if (msg.command & IPMSG_FILEATTACHOPT) flags += "FILEATTACHOPT ";
                    if (msg.command & IPMSG_UTF8OPT) flags += "UTF8OPT ";
                    if (msg.command & IPMSG_CAPUTF8OPT) flags += "CAPUTF8OPT ";
                    LogMessage("BRIDGE", "", "[GUI-MSG] command_flags: " + flags);
                }
                LogMessage("BRIDGE", "", std::string("[GUI-MSG] body=\"") + msg.body + "\" (len=" + std::to_string(msg.body.size()) + ")");
                LogMessage("BRIDGE", "", std::string("[GUI-MSG] extra=\"") + msg.extra + "\" (len=" + std::to_string(msg.extra.size()) + ")");
                {
                    std::ostringstream hexOs;
                    hexOs << std::hex << std::setfill('0') << std::setw(2);
                    for (size_t i = 0; i < msg.extra.size() && i < 200; ++i) {
                        hexOs << (unsigned int)(unsigned char)msg.extra[i] << " ";
                    }
                    LogMessage("BRIDGE", "", "[GUI-MSG] extra_hex: " + hexOs.str());
                }
                LogMessage("BRIDGE", "", "[GUI-MSG] ====== END MESSAGE ======");
                LogMessage("BRIDGE", "", std::string("[GUI-MSG] from=") + msg.sender.userName + "@" +
                              msg.sender.ipAddress + ":" + std::to_string(msg.sender.portNo) +
                              " cmd=" + cmdBuf + " mode=" + modeStr +
                              " body=\"" + msg.body + "\" extra=\"" + msg.extra + "\"");
            }

            // For RECVMSG (acknowledgment), forward to frontend as message status update
            uint32_t mode = GET_MODE(msg.command);
            if (mode == IPMSG_RECVMSG) {
                nlohmann::json ack = {
                    {"packetNo", msg.packetNo},
                    {"from", msg.sender.Key()}
                };
                bridge_->Emit("message.ack", ack);
                return;
            }

            // Ignore our own broadcasted messages (including our own screenshot echoes)
            if (msg.sender.Key() == msgMng_->GetLocalUser().Key()) {
                return;
            }

            // --- FeiQ inline screenshot (custom fragmented image protocol) ---
            // Reference message: body starts with "/~#>" and embeds the screenshot id.
            if (!msg.body.empty() && msg.body.size() >= 4 && msg.body.compare(0, 4, "/~#>") == 0) {
                this->HandleFeiQScreenshotReference(msg, msg.body);
                // Acknowledge read receipt if FeiQ requested one (SENDCHECKOPT)
                if (msg.command & IPMSG_SENDCHECKOPT) {
                    ipmsg::UserInfo u = msg.sender;
                    msgMng_->SendRecvMsg(u, msg.packetNo);
                }
                return;
            }
            // Fragment message: carries FILEATTACHOPT and body "<hexid>|<size>|<off>|
            // <fragCount>|<fragIndex>|<fragSize>|0|2|0|<mtime>#<data>"
            if ((msg.command & IPMSG_FILEATTACHOPT) != 0 && !msg.body.empty() &&
                msg.body.find('|') != std::string::npos &&
                msg.body.find('#') != std::string::npos &&
                std::isxdigit((unsigned char)msg.body[0])) {
                if (this->HandleFeiQScreenshotFragment(msg)) return;
            }

            // Only handle SENDMSG from here onwards
            if (mode != IPMSG_SENDMSG) return;

            // Ignore our own broadcasted messages. The sender's own socket also
            // receives the SENDMSG it broadcasts, which would otherwise create a
            // spurious incoming "file receive request" for our own outgoing file.
            if (msg.sender.Key() == msgMng_->GetLocalUser().Key()) {
                return;
            }

            // Ensure all sender fields are UTF-8 (FeiQ may send GBK even without UTF8OPT flag)
            auto& sender = const_cast<UserInfo&>(msg.sender);
            if (!(msg.command & IPMSG_UTF8OPT)) {
                // EnsureUtf8 is idempotent - safe to call even if already UTF-8
                sender.nickName = EnsureUtf8(sender.nickName);
                sender.groupName = EnsureUtf8(sender.groupName);
            }

            // Ensure extra is UTF-8 before constructing JSON
            std::string extraUtf8 = EnsureUtf8(msg.extra);

            // Determine message type based on command flags
            bool isFileAttach = (msg.command & IPMSG_FILEATTACHOPT) != 0;

            std::string msgType = "text";
            int dbType = 0;  // 0:text, 1:image, 2:file
            std::string fileName;
            int64_t fileSize = 0;
            int fileId = 0;

            if (isFileAttach && !extraUtf8.empty()) {
                // Parse file attachment info in IPMsg/Feiq format:
                // "fileId:filename:hexSize:hexMtime:hexFileType:\a"
                // Multiple files are separated by \a (0x07)
                // Colons in filenames are escaped as ::
                // Reference: Feiq feiqengine.cpp RecvFile::createFileContent

                // Parse fields from extra, handling :: escape and \a separator
                auto parseFileAttachInfo = [](const std::string& extra,
                                             int& outFileId, std::string& outFileName,
                                             int64_t& outFileSize) {
                    // Split by \a (0x07) for multiple files - take first file only
                    std::string fileInfo = extra;
                    auto sepPos = extra.find('\x07');
                    if (sepPos != std::string::npos) {
                        fileInfo = extra.substr(0, sepPos);
                    }

                    // Parse colon-separated fields, with :: escape
                    std::vector<std::string> fields;
                    std::string current;
                    for (size_t i = 0; i < fileInfo.size(); i++) {
                        if (fileInfo[i] == ':') {
                            if (i + 1 < fileInfo.size() && fileInfo[i + 1] == ':') {
                                // Escaped colon ::
                                current += ':';
                                i++; // skip next colon
                            } else {
                                // Field separator
                                fields.push_back(current);
                                current.clear();
                            }
                        } else {
                            current += fileInfo[i];
                        }
                    }
                    fields.push_back(current); // last field

                    // Need at least 3 fields: fileId, filename, size
                    if (fields.size() < 3) return;

                    // IPMsg format: "fileNo:filename:hexSize:hexMtime:hexFileAttr[:extend-attr]"
                    // Field 0: fileNo (decimal) - used as fileId in GETFILEDATA
                    try { outFileId = std::stoi(fields[0]); } catch (...) {}

                    // Field 1: filename
                    outFileName = fields[1];

                    // Field 2: fileSize (hexadecimal)
                    try { outFileSize = std::stoll(fields[2], nullptr, 16); } catch (...) {}

                    // Field 3: mtime (hex, NOT used for GETFILEDATA)
                    // Field 4: fileAttr (hex, e.g. IPMSG_FILE_REGULAR=1)
                };

                parseFileAttachInfo(msg.extra, fileId, fileName, fileSize);

                // Log the raw extra bytes for comparison with our sending format
                {
                    std::ostringstream extraDbg;
                    extraDbg << "[RECV-FILE-EXTRA] Raw extra (" << msg.extra.size() << " bytes): ";
                    for (size_t i = 0; i < msg.extra.size() && i < 200; i++) {
                        unsigned char c = static_cast<unsigned char>(msg.extra[i]);
                        if (c == '\0') extraDbg << "\\0";
                        else if (c == '\x07') extraDbg << "\\a";
                        else if (c == '\n') extraDbg << "\\n";
                        else if (c >= 32 && c < 127) extraDbg << c;
                        else extraDbg << "<" << std::hex << (int)c << ">";
                    }
                    LogMessage("BRIDGE", "", extraDbg.str());
                }
                {
                    std::ostringstream detailOs;
                    detailOs << std::dec << "[RECV-FILE-EXTRA] Parsed: fileId=" << fileId 
                             << ", fileName=" << fileName << ", fileSize=" << fileSize;
                    LogMessage("BRIDGE", "", detailOs.str());
                }
                {
                    std::ostringstream detailOs;
                    detailOs << "[RECV-FILE-EXTRA] Original msg: packetNo=" << msg.packetNo 
                             << ", cmd=0x" << std::hex << msg.command << std::dec
                             << ", body='" << msg.body << "'";
                    LogMessage("BRIDGE", "", detailOs.str());
                }

                // Determine if image or file based on extension
                std::string ext = fileName;
                auto dotPos = ext.find_last_of('.');
                if (dotPos != std::string::npos) {
                    ext = ext.substr(dotPos + 1);
                } else {
                    ext.clear();
                }
                std::transform(ext.begin(), ext.end(), ext.begin(), ::tolower);

                if (ext == "png" || ext == "jpg" || ext == "jpeg" || ext == "gif" ||
                    ext == "bmp" || ext == "webp") {
                    msgType = "image";
                    dbType = 1;
                } else {
                    msgType = "file";
                    dbType = 2;
                }
            }

            nlohmann::json j = {
                {"id", std::to_string(msg.packetNo)},
                {"from", msg.sender.Key()},
                {"fromUser", UserToJson(msg.sender)},
                {"content", msg.body},
                {"type", msgType},
                {"timestamp", static_cast<int64_t>(msg.timestamp)},
                {"command", msg.command},
                {"extra", extraUtf8}
            };

            // Save to database (skip file attachments - they're managed by frontend via file.receive_request)
            // File attachment messages will be saved when the transfer completes
            if (!isFileAttach) {
                MessageRecord record;
                record.id = std::to_string(msg.packetNo);
                record.fromId = msg.sender.Key();
                record.toId = msgMng_->GetLocalUser().Key();
                record.content = msg.body;
                record.type = dbType;
                record.timestamp = static_cast<int64_t>(msg.timestamp);
                record.status = 1; // delivered
                msgDb_->SaveMessage(record);
            }

            // Emit message received event
            bridge_->Emit("message.received", j);

            // Flash tray icon for new text messages (when window is hidden/minimized)
            if (window_ && !window_->IsVisible()) {
                window_->ShowTrayNotification("新消息", "");
            }

            // Play notification sound for new text messages
            if (notificationSound_) {
                PlayNotificationSound();
            }

            // Do NOT auto-reply RECVMSG for file attachment notifications
            // RECVMSG should be sent when user clicks "Accept", not when notification is received
            // FeiQ interprets RECVMSG as "user accepted the file transfer"
            // If (isFileAttach) { ... }

            // If file attachment, emit file receive request event (NOT auto-accepting)
            if (isFileAttach && !fileName.empty()) {
                LogMessage("BRIDGE", "", "[FILE_REQ_EMIT] packetNo=" + std::to_string(msg.packetNo) +
                              ", fromUser=" + msg.sender.Key() +
                              ", fromIp=" + msg.sender.ipAddress +
                              ", fromPort=" + std::to_string(msg.sender.portNo) +
                              ", fileName=" + fileName + 
                              ", fileSize=" + std::to_string(fileSize) +
                              ", fileId=" + std::to_string(fileId) +
                              ", transferId=" + std::to_string(msg.packetNo));
                
                LogMessage("BRIDGE", "", "[BRIDGE_EMIT] Emitting file.receive_request event");
                bridge_->Emit("file.receive_request", {
                    {"packetNo", msg.packetNo},
                    {"fromUser", msg.sender.Key()},
                    {"fromUserIp", msg.sender.ipAddress},
                    {"fromUserPort", msg.sender.portNo},
                    // 接收方文件名来自协议（GBK），转 UTF-8 供前端正确显示
                    {"fileName", EnsureUtf8(fileName)},
                    {"fileSize", fileSize},
                    {"fileId", fileId},
                    {"transferId", std::to_string(msg.packetNo)}
                });

                // Flash tray icon for file receive request (when window is hidden/minimized)
                if (window_ && !window_->IsVisible()) {
                    window_->ShowTrayNotification("文件接收", "");
                }

                // Play notification sound for file receive request
                if (notificationSound_) {
                    PlayNotificationSound();
                }
            }
        } catch (const std::exception& e) {
            LogMessage("BRIDGE", "", std::string("[GUI-MSG] Exception in message callback: ") + e.what());
        } catch (...) {
            LogMessage("BRIDGE", "", "[GUI-MSG] Unknown exception in message callback");
        }
    });

    msgMng_->SetUserStatusChangedCallback([this](const UserInfo& user) {
        bool isAway = (user.hostStatus & IPMSG_ABSENCEOPT) != 0;
        bridge_->Emit("user.status_changed", {
            {"user", UserToJson(user)},
            {"status", isAway ? "away" : "online"}
        });
    });
}

// ---------- User Commands ----------

nlohmann::json CommandHandler::HandleUserDiscover(const nlohmann::json& args) {
    msgMng_->BroadcastEntry();
    return {{"success", true}};
}

nlohmann::json CommandHandler::HandleUserList(const nlohmann::json& args) {
    auto users = msgMng_->GetUsers();
    nlohmann::json userList = nlohmann::json::array();
    for (const auto& u : users) {
        userList.push_back(UserToJson(u));
    }
    return {{"users", userList}, {"count", users.size()}};
}

nlohmann::json CommandHandler::HandleUserStatus(const nlohmann::json& args) {
    std::string status = args.value("status", "online");
    uint32_t cmd = (status == "away") ?
        (IPMSG_BR_ABSENCE | IPMSG_ABSENCEOPT) :
        IPMSG_BR_ABSENCE;
    msgMng_->BroadcastAbsence(cmd);
    return {{"success", true}, {"status", status}};
}

nlohmann::json CommandHandler::HandleUserLocal(const nlohmann::json& args) {
    auto& localUser = msgMng_->GetLocalUser();
    return {
        {"success", true},
        {"id", localUser.Key()},
        {"nickname", localUser.nickName},
        {"username", localUser.userName},
        {"hostname", localUser.hostName},
        {"group", localUser.groupName},
        {"ip", localUser.ipAddress},
        {"port", localUser.portNo}
    };
}

nlohmann::json CommandHandler::HandleConfigSet(const nlohmann::json& args) {
    std::string nickname = args.value("nickname", "");
    std::string group = args.value("group", "");
    std::string dataDir = args.value("dataDir", "");
    std::string minimizeBehavior = args.value("minimizeBehavior", "");
    
    if (!nickname.empty() || !group.empty()) {
        msgMng_->UpdateLocalInfo(nickname, group);
        LogMessage("BRIDGE", "", "[BACKEND-CONFIG] Updated: nickname=" + nickname + ", group=" + group);
        LogMessage("BRIDGE", "", "Config updated: nickname=" + nickname + ", group=" + group);
    }

    // Store custom data directory (used for downloads etc.)
    if (!dataDir.empty()) {
        dataDir_ = dataDir;
        CreateDirectoryA(dataDir_.c_str(), nullptr);
        LogMessage("BRIDGE", "", "Config updated: dataDir=" + dataDir_);
    } else if (args.contains("dataDir") && args["dataDir"].is_string() && args["dataDir"].get<std::string>().empty()) {
        // dataDir explicitly set to empty -> reset to default
        dataDir_.clear();
        LogMessage("BRIDGE", "", "Config updated: dataDir reset to default");
    }

    // Store minimize behavior setting
    if (args.contains("minimizeBehavior")) {
        minimizeBehavior_ = args.value("minimizeBehavior", "taskbar");
        LogMessage("BRIDGE", "", "Config updated: minimizeBehavior=" + minimizeBehavior_);
    }

    // Store notification sound setting
    if (args.contains("notificationSound")) {
        notificationSound_ = args.value("notificationSound", true);
        LogMessage("BRIDGE", "", "Config updated: notificationSound=" + std::string(notificationSound_ ? "true" : "false"));
    }

    // Sync custom broadcast segments from frontend config
    if (args.contains("segments") && args["segments"].is_array()) {
        // Clear existing custom segments (auto-detected ones are rebuilt by GetAllBroadcastAddresses)
        auto current = msgMng_->GetSegments();
        for (const auto& seg : current) {
            msgMng_->RemoveSegment(seg);
        }
        // Add new segments from config
        for (const auto& seg : args["segments"]) {
            if (seg.is_string()) {
                msgMng_->AddSegment(seg.get<std::string>());
            }
        }
        LogMessage("BRIDGE", "", "Config updated: segments synced (" +
                   std::to_string(args["segments"].size()) + " entries)");
    }

    return {{"success", true}};
}

// ---------- Message Commands ----------

nlohmann::json CommandHandler::HandleMessageSend(const nlohmann::json& args) {
    auto target = FindUserFromArgs(args);
    if (!target) {
        return {{"success", false}, {"error", "Target user not found"}};
    }

    std::string content = args.value("content", "");
    if (content.empty()) {
        return {{"success", false}, {"error", "Message content is empty"}};
    }

    LogMessage("BRIDGE", "", "[BACKEND-SEND] TEXT to=" + target->Key() + ", content=\"" + content + "\"");

    // Normal mode: send via UDP
    // Try UTF-8 first (with IPMSG_UTF8OPT flag), fallback to GBK if needed.
    // Feiq/FeiQ handles IPMSG_UTF8OPT properly for UTF-8 encoded content.
    // For other IPMsg clients that don't understand UTF8OPT (like older FeiQ),
    // we send GBK encoded content without the UTF8 flag.
    bool ok = msgMng_->SendMessage(*target, content, IPMSG_SENDCHECKOPT);

    // Always save to database (even if send failed, we want to track it)
    MessageRecord record;
    {
        // Generate unique ID: timestamp_ms + random suffix to avoid collision
        auto now = std::chrono::system_clock::now().time_since_epoch().count();
        std::random_device rd;
        std::mt19937 gen(rd());
        std::uniform_int_distribution<> dis(1000, 9999);
        record.id = std::to_string(now) + "_" + std::to_string(dis(gen));
    }
    record.fromId = msgMng_->GetLocalUser().Key();
    record.toId = target->Key();
    record.content = content;
    record.type = 0;  // text
    record.timestamp = static_cast<int64_t>(std::time(nullptr));
    record.status = ok ? 0 : 2; // 0=sending, 2=failed
    msgDb_->SaveMessage(record);

    return {{"success", ok}, {"messageId", record.id}};
}

nlohmann::json CommandHandler::HandleMessageSendImage(const nlohmann::json& args) {
    auto target = FindUserFromArgs(args);
    if (!target) {
        return {{"success", false}, {"error", "Target user not found"}};
    }

    std::string filePath = args.value("filePath", "");
    if (filePath.empty()) {
        return {{"success", false}, {"error", "Image file path is empty"}};
    }

    LogMessage("BRIDGE", "", "[BACKEND-SEND] IMAGE to=" + target->Key() + ", filePath=\"" + filePath + "\"");

    // Start TCP file transfer (register file info for serving)
    std::string transferId = fileTransfer_->StartSendFile(
        target->ipAddress, target->portNo, filePath, target->Key());

    if (transferId.empty()) {
        return {{"success", false}, {"error", "Failed to start file transfer"}};
    }

    // Get file info
    auto fileInfo = fileTransfer_->GetFileInfo(transferId);
    if (!fileInfo) {
        return {{"success", false}, {"error", "Failed to get file info"}};
    }

    // Build file attach info for IPMsg protocol (Feiq format):
    // "fileId:filename:hexSize:hexMtime:hexFileType:\a"
    std::ostringstream attachOs;
    // 协议层文件名必须是 GBK（飞秋/原生 UI 按 ANSI 解析），内部 fileInfo->fileName 是 UTF-8
    std::string escapedFileName = Utf8ToGbk(fileInfo->fileName);
    // Escape colons in filename (:: represents a literal colon)
    {
        std::string escaped;
        for (char c : escapedFileName) {
            if (c == ':') escaped += "::";
            else escaped += c;
        }
        escapedFileName = escaped;
    }
    attachOs << fileInfo->fileId << ":" << escapedFileName << ":"
             << std::hex << fileInfo->fileSize << ":"
             << fileInfo->modifyTime << ":"
             << fileInfo->fileAttr << ":\x07";
    std::string fileAttachInfo = attachOs.str();

    // Debug: print the file attach info with visible control chars
    {
        std::ostringstream dbgOs;
        for (size_t i = 0; i < fileAttachInfo.size(); i++) {
            unsigned char c = static_cast<unsigned char>(fileAttachInfo[i]);
            if (c == '\0') dbgOs << "\\0";
            else if (c == '\x07') dbgOs << "\\a";
            else if (c == '\n') dbgOs << "\\n";
            else if (c >= 32 && c < 127) dbgOs << c;
            else dbgOs << "<" << std::hex << (int)c << ">";
        }
        LogMessage("BRIDGE", "", "[SEND-FILE-EXTRA] " + dbgOs.str());
        std::ostringstream detailOs;
        detailOs << std::dec << "[SEND-FILE-EXTRA] fileId=" << fileInfo->fileId 
                  << ", fileName=" << fileInfo->fileName
                  << ", fileSize=" << fileInfo->fileSize
                  << ", modifyTime=" << fileInfo->modifyTime
                  << ", fileAttr=" << fileInfo->fileAttr;
        LogMessage("BRIDGE", "", detailOs.str());
    }

    // Normal mode: send UDP notification with file attachment info
    uint64_t sentPktNo = msgMng_->SendMessageWithFile(*target, "[Image: " + fileInfo->fileName + "]", fileAttachInfo);

    if (sentPktNo > 0) {
        // Store the SENDMSG packetNo in FileInfo for matching GETFILEDATA requests
        {
            auto fi = fileTransfer_->GetFileInfo(transferId);
            if (fi) {
                fi->packetNo = sentPktNo;
                fileTransfer_->RegisterFileInfo(transferId, *fi);
            }
        }
        // Save to database
        MessageRecord record;
        record.id = transferId;
        record.fromId = msgMng_->GetLocalUser().Key();
        record.toId = target->Key();
        record.content = filePath;  // Store file path
        record.type = 1;  // image
        record.timestamp = static_cast<int64_t>(std::time(nullptr));
        record.status = 1; // sending
        msgDb_->SaveMessage(record);

        // Emit transfer started event
        bridge_->Emit("file.transfer_started", {
            {"transferId", transferId},
            {"filename", fileInfo->fileName},
            {"fileSize", fileInfo->fileSize},
            {"isSending", true},
            {"targetUser", target->Key()}
        });
    }

    return {{"success", true}, {"transferId", transferId}, {"fileName", fileInfo->fileName}};
}

// ---------- File Commands ----------

nlohmann::json CommandHandler::HandleFileSend(const nlohmann::json& args) {
    auto target = FindUserFromArgs(args);
    if (!target) {
        return {{"success", false}, {"error", "Target user not found"}};
    }

    std::string filePath = args.value("filePath", "");
    if (filePath.empty()) {
        return {{"success", false}, {"error", "File path is empty"}};
    }

    LogMessage("BRIDGE", "", "[BACKEND-SEND] FILE to=" + target->Key() + ", filePath=\"" + filePath + "\"");

    // Start TCP file transfer (register file info for serving)
    std::string transferId = fileTransfer_->StartSendFile(
        target->ipAddress, target->portNo, filePath, target->Key());

    if (transferId.empty()) {
        return {{"success", false}, {"error", "Failed to start file transfer"}};
    }

    // Get file info
    auto fileInfo = fileTransfer_->GetFileInfo(transferId);
    if (!fileInfo) {
        return {{"success", false}, {"error", "Failed to get file info"}};
    }

    // Build file attach info for IPMsg protocol (Feiq format):
    // "fileId:filename:hexSize:hexMtime:hexFileType:\a"
    std::ostringstream attachOs;
    // 协议层文件名必须是 GBK（飞秋/原生 UI 按 ANSI 解析），内部 fileInfo->fileName 是 UTF-8
    std::string escapedFileName = Utf8ToGbk(fileInfo->fileName);
    // Escape colons in filename (:: represents a literal colon)
    {
        std::string escaped;
        for (char c : escapedFileName) {
            if (c == ':') escaped += "::";
            else escaped += c;
        }
        escapedFileName = escaped;
    }
    attachOs << fileInfo->fileId << ":" << escapedFileName << ":"
             << std::hex << fileInfo->fileSize << ":"
             << fileInfo->modifyTime << ":"
             << fileInfo->fileAttr << ":\x07";
    std::string fileAttachInfo = attachOs.str();

    // Debug: print the file attach info with visible control chars
    {
        std::ostringstream dbgOs;
        for (size_t i = 0; i < fileAttachInfo.size(); i++) {
            unsigned char c = static_cast<unsigned char>(fileAttachInfo[i]);
            if (c == '\0') dbgOs << "\\0";
            else if (c == '\x07') dbgOs << "\\a";
            else if (c == '\n') dbgOs << "\\n";
            else if (c >= 32 && c < 127) dbgOs << c;
            else dbgOs << "<" << std::hex << (int)c << ">";
        }
        LogMessage("BRIDGE", "", "[SEND-FILE-EXTRA] " + dbgOs.str());
        std::ostringstream detailOs;
        detailOs << std::dec << "[SEND-FILE-EXTRA] fileId=" << fileInfo->fileId 
                  << ", fileName=" << fileInfo->fileName
                  << ", fileSize=" << fileInfo->fileSize
                  << ", modifyTime=" << fileInfo->modifyTime
                  << ", fileAttr=" << fileInfo->fileAttr;
        LogMessage("BRIDGE", "", detailOs.str());
    }

    // Normal mode: send UDP notification with file attachment info
    LogMessage("BRIDGE", "", "[BACKEND-SEND] Sending SENDMSG with FILEATTACHOPT to " + target->Key() +
                  " (fileId=" + std::to_string(fileInfo->fileId) + ", fileSize=" + std::to_string(fileInfo->fileSize) + ")");
    LogMessage("BRIDGE", "", "[BACKEND-SEND] File attach info: " + fileAttachInfo);
    
    // 通知消息文本里的文件名也用 GBK，避免飞秋消息列表里乱码
    uint64_t sentPktNo = msgMng_->SendMessageWithFile(*target, "[File: " + Utf8ToGbk(fileInfo->fileName) + "]", fileAttachInfo, IPMSG_SENDCHECKOPT);

    if (sentPktNo > 0) {
        LogMessage("BRIDGE", "", "[BACKEND-SEND] SENDMSG sent successfully, packetNo=" + std::to_string(sentPktNo));
        // Store the SENDMSG packetNo in FileInfo for matching GETFILEDATA requests
        {
            auto fi = fileTransfer_->GetFileInfo(transferId);
            if (fi) {
                fi->packetNo = sentPktNo;
                fileTransfer_->RegisterFileInfo(transferId, *fi);
                LogMessage("BRIDGE", "", "[BACKEND-SEND] FileInfo updated: packetNo=" + std::to_string(sentPktNo) + ", fileId=" + std::to_string(fi->fileId));
            }
        }
        // Save to database
        MessageRecord record;
        record.id = transferId;
        record.fromId = msgMng_->GetLocalUser().Key();
        record.toId = target->Key();
        record.content = filePath;  // Store file path
        record.type = 2;  // file
        record.timestamp = static_cast<int64_t>(std::time(nullptr));
        record.status = 1; // sending
        msgDb_->SaveMessage(record);

        // Emit transfer started event
        bridge_->Emit("file.transfer_started", {
            {"transferId", transferId},
            {"filename", fileInfo->fileName},
            {"fileSize", fileInfo->fileSize},
            {"isSending", true},
            {"targetUser", target->Key()}
        });
    }

    return {{"success", true}, {"transferId", transferId}, {"fileName", fileInfo->fileName}};
}

nlohmann::json CommandHandler::HandleFileInfo(const nlohmann::json& args) {
    std::string filePath = args.value("filePath", "");
    if (filePath.empty()) {
        return {{"success", false}, {"error", "File path is empty"}};
    }
    try {
        std::error_code ec;
        uint64_t size = fs::file_size(fs::u8path(filePath), ec);
        if (ec) {
            return {{"success", false}, {"error", ec.message()}};
        }
        std::string name = fs::u8path(filePath).filename().u8string();
        return {{"success", true}, {"fileSize", size}, {"fileName", name}};
    } catch (const std::exception& e) {
        return {{"success", false}, {"error", std::string(e.what())}};
    }
}

nlohmann::json CommandHandler::HandleFileRecv(const nlohmann::json& args) {
    auto target = FindUserFromArgs(args);
    if (!target) {
        return {{"success", false}, {"error", "Target user not found"}};
    }

    std::string transferId = args.value("transferId", "");
    std::string fileName = args.value("fileName", "");
    int64_t fileSize = args.value("fileSize", 0);
    std::string savePath = args.value("savePath", "");
    uint64_t origPacketNo = args.value("packetNo", (uint64_t)0);
    int origFileId = args.value("fileId", 0);

    if (transferId.empty() || fileName.empty() || savePath.empty()) {
        return {{"success", false}, {"error", "Missing required parameters"}};
    }

    LogMessage("BRIDGE", "", "[BACKEND-RECV] FILE from=" + target->Key() + ", fileName=\"" + fileName + "\", fileSize=" + std::to_string(fileSize) + ", savePath=\"" + savePath + "\"");

    // Start receiving file via TCP (same port as UDP per IPMsg protocol)
    std::string recvTransferId = fileTransfer_->StartRecvFile(
        target->ipAddress, target->portNo, fileName, fileSize, savePath, target->Key(),
        origPacketNo, origFileId);

    if (recvTransferId.empty()) {
        return {{"success", false}, {"error", "Failed to start file receive"}};
    }

    // Save to database
    MessageRecord record;
    record.id = recvTransferId;
    record.fromId = target->Key();
    record.toId = msgMng_->GetLocalUser().Key();
    record.content = savePath;  // Store save path
    record.type = 2;  // file
    record.timestamp = static_cast<int64_t>(std::time(nullptr));
    record.status = 1; // receiving
    msgDb_->SaveMessage(record);

    // Emit transfer started event
    bridge_->Emit("file.transfer_started", {
        {"transferId", recvTransferId},
        {"filename", fileName},
        {"fileSize", fileSize},
        {"isSending", false},
        {"targetUser", target->Key()},
        {"savePath", savePath}
    });

    return {{"success", true}, {"transferId", recvTransferId}};
}

nlohmann::json CommandHandler::HandleFileSaveTemp(const nlohmann::json& args) {
    std::string base64Data = args.value("data", "");
    std::string filename = args.value("filename", "temp_file");

    if (base64Data.empty()) {
        return {{"success", false}, {"error", "Base64 data is empty"}};
    }

    // Decode base64
    std::string decodedData;
    try {
        // Simple base64 decode implementation
        static const std::string base64_chars =
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
            "abcdefghijklmnopqrstuvwxyz"
            "0123456789+/";

        int in_len = base64Data.size();
        int i = 0, j = 0;
        char char_array_4[4], char_array_3[3];

        while (in_len-- && base64Data[i] != '=' &&
               (isalnum(base64Data[i]) || base64Data[i] == '+' || base64Data[i] == '/')) {
            char_array_4[j++] = base64Data[i]; i++;
            if (j == 4) {
                for (j = 0; j < 4; j++)
                    char_array_4[j] = base64_chars.find(char_array_4[j]);
                char_array_3[0] = (char_array_4[0] << 2) + ((char_array_4[1] & 0x30) >> 4);
                char_array_3[1] = ((char_array_4[1] & 0xf) << 4) + ((char_array_4[2] & 0x3c) >> 2);
                char_array_3[2] = ((char_array_4[2] & 0x3) << 6) + char_array_4[3];
                for (j = 0; j < 3; j++)
                    decodedData += char_array_3[j];
                j = 0;
            }
        }

        if (j) {
            for (int k = j; k < 4; k++)
                char_array_4[k] = 0;
            for (int k = 0; k < 4; k++)
                char_array_4[k] = base64_chars.find(char_array_4[k]);
            char_array_3[0] = (char_array_4[0] << 2) + ((char_array_4[1] & 0x30) >> 4);
            char_array_3[1] = ((char_array_4[1] & 0xf) << 4) + ((char_array_4[2] & 0x3c) >> 2);
            char_array_3[2] = ((char_array_4[2] & 0x3) << 6) + char_array_4[3];
            for (int k = 0; k < j - 1; k++)
                decodedData += char_array_3[k];
        }
    } catch (...) {
        return {{"success", false}, {"error", "Failed to decode base64 data"}};
    }

    // Create temp directory
    char tempPath[MAX_PATH];
    GetTempPathA(MAX_PATH, tempPath);
    std::string tempDir = std::string(tempPath) + "IPMsgPro";
    if (!CreateDirectoryA(tempDir.c_str(), nullptr)) {
        DWORD err = GetLastError();
        if (err != ERROR_ALREADY_EXISTS) {
            return {{"success", false}, {"error", "Failed to create temp dir: " + std::to_string(err)}};
        }
    }

    // Sanitize filename - remove path separators and special chars
    std::string safeFilename = filename;
    for (auto& c : safeFilename) {
        if (c == '/' || c == '\\' || c == ':' || c == '*' || c == '?' || c == '"' ||
            c == '<' || c == '>' || c == '|') {
            c = '_';
        }
    }
    if (safeFilename.empty()) safeFilename = "temp_file";

    // Generate unique filename
    std::string tempFile = tempDir + "\\" + std::to_string(std::time(nullptr)) + "_" + safeFilename;

    // Write to file
    std::ofstream outFile(tempFile, std::ios::binary);
    if (!outFile.is_open()) {
        DWORD err = GetLastError();
        return {{"success", false}, {"error", "Failed to create temp file: " + std::string(tempFile) + " err=" + std::to_string(err)}};
    }

    outFile.write(decodedData.data(), decodedData.size());
    outFile.close();

    return {{"success", true}, {"filePath", tempFile}};
}

nlohmann::json CommandHandler::HandleFileAccept(const nlohmann::json& args) {
    LogMessage("BRIDGE", "", "[BACKEND-ACCEPT-ENTRY] file.accept called with args: " + args.dump());

    auto target = FindUserFromArgs(args);
    if (!target) {
        LogMessage("BRIDGE", "", "[BACKEND-ACCEPT-ERROR] Target user not found! args=" + args.dump());
        return {{"success", false}, {"error", "Target user not found"}};
    }
    LogMessage("BRIDGE", "", "[BACKEND-ACCEPT] Found target user: " + target->Key() + ", ip=" + target->ipAddress + ", port=" + std::to_string(target->portNo));

    std::string transferId = args.value("transferId", "");
    std::string fileName = args.value("fileName", "");
    int64_t fileSize = args.value("fileSize", 0);
    std::string savePath = args.value("savePath", "");
    uint64_t origPacketNo = args.value("packetNo", (uint64_t)0);
    int origFileId = args.value("fileId", 0);

    // If savePath is empty, auto-generate using the user's Downloads folder
    if (savePath.empty() && !fileName.empty()) {
        std::string saveDir = GetUserDownloadsDir();
        CreateDirectoryW(Utf8ToWide(saveDir).c_str(), nullptr);
        savePath = saveDir + "\\" + fileName;
    }
    LogMessage("BRIDGE", "", "[BACKEND-ACCEPT] savePath=" + savePath);

    if (transferId.empty() || fileName.empty() || savePath.empty()) {
        LogMessage("BRIDGE", "", "[BACKEND-ACCEPT-ERROR] Missing required parameters!");
        return {{"success", false}, {"error", "Missing required parameters"}};
    }

    LogMessage("BRIDGE", "", "[BACKEND-ACCEPT] Calling StartRecvFile: ip=" + target->ipAddress + ", port=" + std::to_string(target->portNo) + ", fileName=" + fileName + ", fileSize=" + std::to_string(fileSize) + ", savePath=" + savePath + ", origPacketNo=" + std::to_string(origPacketNo) + ", origFileId=" + std::to_string(origFileId));

    // Send IPMSG_RECVMSG acknowledgment to sender (delivery receipt)
    // Use the original SENDMSG packetNo (not the internal transferId)
    if (origPacketNo > 0) {
        msgMng_->SendRecvMsg(*target, origPacketNo);
    }

    // Normal mode: Start receiving file via TCP (same port as UDP per IPMsg protocol)
    // Pass origPacketNo and origFileId for building the GETFILEDATA request
    std::string recvTransferId = fileTransfer_->StartRecvFile(
        target->ipAddress, target->portNo, fileName, fileSize, savePath, target->Key(),
        origPacketNo, origFileId);

    LogMessage("BRIDGE", "", "[BACKEND-ACCEPT] StartRecvFile result: recvTransferId=" + recvTransferId);

    if (recvTransferId.empty()) {
        LogMessage("BRIDGE", "", "[BACKEND-ACCEPT-ERROR] StartRecvFile failed!");
        return {{"success", false}, {"error", "Failed to start file receive"}};
    }

    // Save to database
    MessageRecord record;
    record.id = recvTransferId;
    record.fromId = target->Key();
    record.toId = msgMng_->GetLocalUser().Key();
    record.content = savePath;
    record.type = 2;  // file
    record.timestamp = static_cast<int64_t>(std::time(nullptr));
    record.status = 1; // receiving
    msgDb_->SaveMessage(record);

    // Emit transfer started event
    bridge_->Emit("file.transfer_started", {
        {"transferId", recvTransferId},
        {"filename", fileName},
        {"fileSize", fileSize},
        {"isSending", false},
        {"targetUser", target->Key()},
        {"savePath", savePath}
    });

    return {{"success", true}, {"transferId", recvTransferId}};
}

nlohmann::json CommandHandler::HandleFileReject(const nlohmann::json& args) {
    auto target = FindUserFromArgs(args);
    if (!target) {
        return {{"success", false}, {"error", "Target user not found"}};
    }

    std::string transferId = args.value("transferId", "");

    // Send IPMSG_RELEASEFILES to sender to release the shared files
    // In IPMsg protocol, RELEASEFILES uses the same packetNo as the original SENDMSG
    uint64_t packetNo = 0;
    try { packetNo = std::stoull(transferId); } catch (...) {}

    if (packetNo > 0) {
        // Send RELEASEFILES command to notify sender
        std::string extra = std::to_string(packetNo);
        msgMng_->SendMessage(*target, extra,
            IPMSG_RELEASEFILES | IPMSG_FILEATTACHOPT);
    }

    return {{"success", true}};
}

nlohmann::json CommandHandler::HandleShellOpen(const nlohmann::json& args) {
    std::string url = args.value("url", "");
    if (url.empty()) {
        return {{"success", false}, {"error", "URL is empty"}};
    }

    // Open the URL in the user's default browser via ShellExecuteW.
    // The URL is UTF-8; convert it to UTF-16 so it works regardless of locale.
    std::wstring wUrl = Utf8ToWide(url);
    if (wUrl.empty()) {
        return {{"success", false}, {"error", "Invalid URL encoding"}};
    }

    HINSTANCE result = ShellExecuteW(
        nullptr,
        L"open",
        wUrl.c_str(),
        nullptr,
        nullptr,
        SW_SHOWNORMAL
    );

    // ShellExecuteW returns > 32 on success
    bool ok = reinterpret_cast<INT_PTR>(result) > 32;
    return {{"success", ok}};
}

// ============================================================================
// Screenshot support
// ============================================================================

// Find the PNG image encoder CLSID.
static bool GetPngEncoderClsid(CLSID* clsid) {
    UINT num = 0, size = 0;
    GetImageEncodersSize(&num, &size);
    if (size == 0) return false;
    std::vector<ImageCodecInfo> encoders(size / sizeof(ImageCodecInfo));
    GetImageEncoders(num, size, encoders.data());
    for (UINT i = 0; i < num; i++) {
        if (wcscmp(encoders[i].MimeType, L"image/png") == 0) {
            *clsid = encoders[i].Clsid;
            return true;
        }
    }
    return false;
}

// Capture a specific monitor to a PNG, returning the raw encoded bytes.
static bool CaptureMonitorToPng(HMONITOR hMonitor, std::vector<BYTE>& outPng) {
    MONITORINFO mi = { sizeof(mi) };
    if (!GetMonitorInfo(hMonitor, &mi)) return false;
    int left = mi.rcMonitor.left, top = mi.rcMonitor.top;
    int w = mi.rcMonitor.right - left;
    int h = mi.rcMonitor.bottom - top;
    if (w <= 0 || h <= 0) return false;

    HDC hdcScreen = GetDC(NULL);
    if (!hdcScreen) return false;

    // Create a 32bpp DIB section so we get reliable pixel access.
    BITMAPINFOHEADER bi = { 0 };
    bi.biSize = sizeof(BITMAPINFOHEADER);
    bi.biWidth = w;
    bi.biHeight = h;          // positive => bottom-up DIB
    bi.biPlanes = 1;
    bi.biBitCount = 32;
    bi.biCompression = BI_RGB;
    void* pBits = nullptr;
    HBITMAP hBmp = CreateDIBSection(hdcScreen, (BITMAPINFO*)&bi, DIB_RGB_COLORS, &pBits, NULL, 0);
    if (!hBmp) {
        ReleaseDC(NULL, hdcScreen);
        return false;
    }

    HDC hdcMem = CreateCompatibleDC(hdcScreen);
    HGDIOBJ old = SelectObject(hdcMem, hBmp);
    BitBlt(hdcMem, 0, 0, w, h, hdcScreen, left, top, SRCCOPY | CAPTUREBLT);
    SelectObject(hdcMem, old);

    // Encode via GDI+.
    GdiplusStartupInput gdiplusStartupInput;
    ULONG_PTR gdiplusToken = 0;
    if (GdiplusStartup(&gdiplusToken, &gdiplusStartupInput, NULL) != Ok) {
        DeleteObject(hBmp);
        DeleteDC(hdcMem);
        ReleaseDC(NULL, hdcScreen);
        return false;
    }

    bool ok = false;
    Bitmap* bmp = new Bitmap(w, h, PixelFormat32bppARGB);
    {
        BitmapData bmpData;
        Rect rect(0, 0, w, h);
        if (bmp->LockBits(&rect, ImageLockModeWrite, PixelFormat32bppARGB, &bmpData) == Ok) {
            int dstStride = bmpData.Stride;
            BYTE* dst = (BYTE*)bmpData.Scan0;
            int srcStride = ((w * 32 + 31) / 32) * 4;
            for (int y = 0; y < h; y++) {
                // DIB is bottom-up: row 0 is the bottom row of the image.
                BYTE* srcRow = (BYTE*)pBits + (h - 1 - y) * srcStride;
                BYTE* dstRow = dst + y * dstStride;
                for (int x = 0; x < w; x++) {
                    BYTE b = srcRow[x * 4 + 0];
                    BYTE g = srcRow[x * 4 + 1];
                    BYTE r = srcRow[x * 4 + 2];
                    BYTE a = srcRow[x * 4 + 3];
                    DWORD pix = ((a ? a : 0xFF) << 24) | (r << 16) | (g << 8) | b;
                    *(DWORD*)(dstRow + x * 4) = pix;
                }
            }
            bmp->UnlockBits(&bmpData);
        }

        CLSID pngClsid;
        if (GetPngEncoderClsid(&pngClsid)) {
            IStream* pStream = NULL;
            if (CreateStreamOnHGlobal(NULL, TRUE, &pStream) == S_OK) {
                if (bmp->Save(pStream, &pngClsid, NULL) == Ok) {
                    HGLOBAL hMem = NULL;
                    if (GetHGlobalFromStream(pStream, &hMem) == S_OK) {
                        SIZE_T size = GlobalSize(hMem);
                        void* pData = GlobalLock(hMem);
                        if (pData) {
                            outPng.assign((BYTE*)pData, (BYTE*)pData + size);
                            ok = true;
                            GlobalUnlock(hMem);
                        }
                    }
                }
                pStream->Release();
            }
        }
    }

    delete bmp;
    GdiplusShutdown(gdiplusToken);
    DeleteObject(hBmp);
    DeleteDC(hdcMem);
    ReleaseDC(NULL, hdcScreen);
    return ok;
}

nlohmann::json CommandHandler::HandleScreenshotCapture(const nlohmann::json& args) {
    (void)args;
    if (!hwnd_) {
        return {{"success", false}, {"error", "Window handle not available"}};
    }
    HWND hWnd = static_cast<HWND>(hwnd_);

    // Determine the monitor that currently hosts the window (before we hide it),
    // so a multi-monitor setup captures the screen the app lives on.
    RECT winRect = { 0 };
    GetWindowRect(hWnd, &winRect);
    int cx = (winRect.left + winRect.right) / 2;
    int cy = (winRect.top + winRect.bottom) / 2;
    HMONITOR hMon = MonitorFromPoint({ cx, cy }, MONITOR_DEFAULTTONEAREST);
    if (!hMon) hMon = MonitorFromWindow(hWnd, MONITOR_DEFAULTTONEAREST);

    MONITORINFO mi = { sizeof(mi) };
    GetMonitorInfo(hMon, &mi);
    int screenCount = GetSystemMetrics(SM_CMONITORS);

    // Hide the window so it does not appear in the capture.
    if (window_) window_->Hide();

    // Give the OS a moment to repaint the desktop without our window.
    RedrawWindow(GetDesktopWindow(), NULL, NULL,
                 RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN);
    Sleep(220);

    std::vector<BYTE> png;
    bool ok = CaptureMonitorToPng(hMon, png);

    if (!ok) {
        // Bring the window back (keep its previous state).
        if (window_) window_->Restore();
        return {{"success", false}, {"error", "Failed to capture screen"}};
    }

    // Return the PNG as a data URL (base64). A data: URL is same-origin, so the
    // frontend can draw it to a canvas and read pixels back (required for
    // cropping / exporting) without tainting the canvas.
    std::string imageDataUrl = "data:image/png;base64," +
        Base64Encode(std::string((const char*)png.data(), png.size()));

    // Bring the window back, maximised, so the editor can go full-screen on the
    // same monitor the screenshot was taken from.
    if (window_) window_->Maximize();

    return {
        {"success", true},
        {"image", imageDataUrl},
        {"monitor", {
            {"x", mi.rcMonitor.left},
            {"y", mi.rcMonitor.top},
            {"width", mi.rcMonitor.right - mi.rcMonitor.left},
            {"height", mi.rcMonitor.bottom - mi.rcMonitor.top}
        }},
        {"screenCount", screenCount}
    };
}

nlohmann::json CommandHandler::HandleWindowMaximize(const nlohmann::json& args) {
    (void)args;
    if (window_) window_->Maximize();
    return {{"success", true}};
}

nlohmann::json CommandHandler::HandleWindowRestore(const nlohmann::json& args) {
    (void)args;
    if (window_) window_->Restore();
    return {{"success", true}};
}

nlohmann::json CommandHandler::HandleDialogSave(const nlohmann::json& args) {
    if (!hwnd_) {
        return {{"success", false}, {"error", "Window handle not available"}};
    }
    HWND hWnd = static_cast<HWND>(hwnd_);
    std::wstring title = Utf8ToWide(args.value("title", "保存截图"));
    std::wstring defaultName = Utf8ToWide(args.value("default_name", "screenshot.png"));

    OPENFILENAMEW ofn = { 0 };
    ofn.lStructSize = sizeof(ofn);
    ofn.hwndOwner = hWnd;
    ofn.lpstrFilter = L"PNG 图片 (*.png)\0*.png\0所有文件 (*.*)\0*.*\0";
    wchar_t szFile[MAX_PATH] = { 0 };
    wcsncpy_s(szFile, defaultName.c_str(), _TRUNCATE);
    ofn.lpstrFile = szFile;
    ofn.nMaxFile = MAX_PATH;
    ofn.lpstrTitle = title.c_str();
    ofn.Flags = OFN_OVERWRITEPROMPT | OFN_NOCHANGEDIR;

    if (GetSaveFileNameW(&ofn)) {
        std::wstring wpath(szFile);
        int len = WideCharToMultiByte(CP_UTF8, 0, wpath.c_str(), -1, NULL, 0, NULL, NULL);
        std::string path(len, '\0');
        WideCharToMultiByte(CP_UTF8, 0, wpath.c_str(), -1, &path[0], len, NULL, NULL);
        if (!path.empty() && path.back() == '\0') path.pop_back();
        return {{"success", true}, {"path", path}};
    }
    return {{"success", false}, {"cancelled", true}};
}

nlohmann::json CommandHandler::HandleFileSaveData(const nlohmann::json& args) {
    std::string base64Data = args.value("data", "");
    std::string path = args.value("path", "");
    if (base64Data.empty() || path.empty()) {
        return {{"success", false}, {"error", "Missing data or path"}};
    }
    std::string decoded = Base64Decode(base64Data);

    // Make sure the parent directory exists.
    fs::path p(path);
    if (auto parent = p.parent_path(); !parent.empty()) {
        std::error_code ec;
        fs::create_directories(parent, ec);
    }

    {
        std::ofstream out(path, std::ios::binary);
        if (!out) {
            return {{"success", false}, {"error", "Cannot open target path"}};
        }
        out.write(decoded.data(), (std::streamsize)decoded.size());
        out.flush();
        if (!out) {
            return {{"success", false}, {"error", "Write failed"}};
        }
    }
    return {{"success", true}, {"path", path}};
}

nlohmann::json CommandHandler::HandleFileOpenFolder(const nlohmann::json& args) {
    std::string path = args.value("path", "");
    if (path.empty()) {
        return {{"success", false}, {"error", "Path is empty"}};
    }

    // Use ShellExecuteW to open explorer and select the file.
    // The path is UTF-8; convert it to UTF-16 properly so Chinese paths work.
    std::wstring wPath = Utf8ToWide(path);
    if (wPath.empty()) {
        return {{"success", false}, {"error", "Invalid path encoding"}};
    }
    std::wstring params = L"/select,\"" + wPath + L"\"";

    HINSTANCE result = ShellExecuteW(
        nullptr,
        L"open",
        L"explorer.exe",
        params.c_str(),
        nullptr,
        SW_SHOWNORMAL
    );

    // ShellExecuteW returns > 32 on success
    bool ok = reinterpret_cast<INT_PTR>(result) > 32;
    return {{"success", ok}};
}

// ---------- History Commands ----------

nlohmann::json CommandHandler::HandleHistoryGet(const nlohmann::json& args) {
    std::string userId = args.value("userId", "");
    int limit = args.value("limit", 50);
    int offset = args.value("offset", 0);

    // Get current user's ID
    std::string localUserId = msgMng_->GetLocalUser().Key();

    std::vector<MessageRecord> messages;
    bool ok = msgDb_->GetMessages(userId, localUserId, limit, offset, messages);

    nlohmann::json msgList = nlohmann::json::array();
    for (const auto& m : messages) {
        msgList.push_back({
            {"id", m.id},
            {"fromId", m.fromId},
            {"toId", m.toId},
            {"content", m.content},
            {"type", m.type},
            {"timestamp", m.timestamp},
            {"status", m.status}
        });
    }

    return {{"success", ok}, {"messages", msgList}, {"localUserId", localUserId}};
}

nlohmann::json CommandHandler::HandleHistorySearch(const nlohmann::json& args) {
    std::string keyword = args.value("keyword", "");
    if (keyword.empty()) {
        return {{"success", false}, {"error", "Keyword is empty"}};
    }

    std::vector<MessageRecord> messages;
    bool ok = msgDb_->SearchMessages(keyword, messages);

    nlohmann::json msgList = nlohmann::json::array();
    for (const auto& m : messages) {
        msgList.push_back({
            {"id", m.id},
            {"fromId", m.fromId},
            {"toId", m.toId},
            {"content", m.content},
            {"type", m.type},
            {"timestamp", m.timestamp},
            {"status", m.status}
        });
    }

    return {{"success", ok}, {"messages", msgList}};
}

nlohmann::json CommandHandler::HandleHistoryClear(const nlohmann::json& args) {
    std::string userId = args.value("userId", "");
    bool ok = msgDb_->ClearMessages(userId);
    return {{"success", ok}};
}

// ---------- Network Commands ----------

nlohmann::json CommandHandler::HandleNetworkScan(const nlohmann::json& args) {
    std::string segment = args.value("segment", "");
    if (!segment.empty()) {
        msgMng_->AddSegment(segment);
    }
    msgMng_->BroadcastEntry();
    return {{"success", true}};
}

// ---------- Helpers ----------

nlohmann::json CommandHandler::UserToJson(const UserInfo& user) {
    std::string status = "online";
    if (!user.active) status = "offline";
    else if (user.hostStatus & IPMSG_ABSENCEOPT) status = "away";

    return {
        {"id", user.Key()},
        {"nickname", user.nickName.empty() ? user.userName : user.nickName},
        {"username", user.userName},
        {"hostname", user.hostName},
        {"group", user.groupName},
        {"ip", user.ipAddress},
        {"port", user.portNo},
        {"status", status},
        {"version", ""}
    };
}

std::optional<UserInfo> CommandHandler::FindUserFromArgs(const nlohmann::json& args) {
    // Try finding by "target" (key or IP)
    if (args.contains("target")) {
        std::string target = args["target"].get<std::string>();

        // Try as key first
        auto user = msgMng_->FindUser(target);
        if (user) return user;

        // Try as IP address
        auto users = msgMng_->GetUsers();
        for (const auto& u : users) {
            if (u.ipAddress == target) return u;
        }
    }
    return std::nullopt;
}

// ---------- Dialog Commands ----------

nlohmann::json CommandHandler::HandleDialogPickFolder(const nlohmann::json& args) {
    std::string title = args.value("title", "Select Folder");
    std::string initialDir = args.value("initial_dir", "");
    LogMessage("BRIDGE", "", "[DIALOG] HandleDialogPickFolder called, title=" + title +
               ", initialDir=" + (initialDir.empty() ? "(default)" : initialDir));

    if (!hwnd_) {
        LogMessage("BRIDGE", "", "[DIALOG] ERROR: hwnd_ is null!");
        return {{"success", false}, {"error", "Window handle not available"}};
    }

    LogMessage("BRIDGE", "", "[DIALOG] hwnd_=" + std::to_string(reinterpret_cast<uintptr_t>(hwnd_)));
    HWND hWnd = static_cast<HWND>(hwnd_);
    LogMessage("BRIDGE", "", "[DIALOG] Calling PickFolder with hWnd=" + std::to_string(reinterpret_cast<uintptr_t>(hWnd)));

    auto folder = tauricpp::Dialog::PickFolder(hWnd, title, initialDir);
    LogMessage("BRIDGE", "", "[DIALOG] PickFolder returned, folder=" + (folder ? *folder : "(empty)"));
    
    if (folder) {
        return {{"success", true}, {"folder", *folder}};
    }
    return {{"success", true}, {"folder", ""}};  // User cancelled
}

nlohmann::json CommandHandler::HandleDialogOpen(const nlohmann::json& args) {
    std::string title = args.value("title", "选择文件");
    bool multi = args.value("multi_select", false);
    LogMessage("BRIDGE", "", "[DIALOG] HandleDialogOpen called, title=" + title);

    if (!hwnd_) {
        LogMessage("BRIDGE", "", "[DIALOG] ERROR: hwnd_ is null!");
        return {{"success", false}, {"error", "Window handle not available"}};
    }

    HWND hWnd = static_cast<HWND>(hwnd_);
    tauricpp::Dialog::OpenOptions opts;
    opts.title = title;
    opts.multi_select = multi;
    if (args.contains("default_path") && args["default_path"].is_string()) {
        opts.default_path = args["default_path"].get<std::string>();
    }

    // 返回 UTF-8 路径，后端的 StartSendFile 用 fs::u8path 正确解析中文路径
    auto files = tauricpp::Dialog::OpenFile(hWnd, opts);
    nlohmann::json arr = nlohmann::json::array();
    for (const auto& f : files) arr.push_back(f);
    return {{"success", true}, {"files", arr}};
}


std::string CommandHandler::GetDataDir() const {
    // If custom dataDir is set, use it; otherwise use default (USERPROFILE\.ipmsgpro)
    if (!dataDir_.empty()) {
        return dataDir_;
    }
    char userProfile[MAX_PATH] = {};
    if (GetEnvironmentVariableA("USERPROFILE", userProfile, MAX_PATH) <= 0) {
        SHGetFolderPathA(nullptr, CSIDL_LOCAL_APPDATA, nullptr, 0, userProfile);
    }
    return std::string(userProfile) + "\\.ipmsgpro";
}

std::string GetUserDownloadsDir() {
    // The user's Downloads folder lives under the user profile directory,
    // e.g. C:\Users\<user>\Downloads. Use USERPROFILE (the documented
    // location) to avoid depending on shell known-folder CSIDL constants
    // that may be unavailable with WIN32_LEAN_AND_MEAN.
    //
    // IMPORTANT: return the path as UTF-8. Downstream code (the receive
    // save-path in RecvFileThread -> PathFromUtf8, and the FeiQ screenshot
    // save -> std::filesystem::u8path) all expect UTF-8. Using the *A (ANSI)
    // variant would yield code-page (GBK) bytes that fail to map for
    // non-ASCII user names (e.g. C:\Users\冯波\Downloads), causing
    // "Access is denied" / "No mapping for the Unicode character" errors.
    wchar_t userProfile[MAX_PATH] = {};
    if (GetEnvironmentVariableW(L"USERPROFILE", userProfile, MAX_PATH) > 0) {
        int len = WideCharToMultiByte(CP_UTF8, 0, userProfile, -1, nullptr, 0, nullptr, nullptr);
        if (len > 0) {
            std::string utf8(len, '\0');
            WideCharToMultiByte(CP_UTF8, 0, userProfile, -1, &utf8[0], len, nullptr, nullptr);
            if (!utf8.empty() && utf8.back() == '\0') utf8.pop_back();
            return utf8 + "\\Downloads";
        }
    }
    return "Downloads";
}

// ---------- FeiQ inline screenshot (custom fragmented image protocol) ----------
//
// FeiQ (飞秋) does NOT send screenshots as a standard IPMsg FILEATTACH. Instead it
// uses a custom inline protocol:
//   1) A reference message (cmd SENDMSG|SENDCHECKOPT) whose body begins with
//      "/~#><id><...>" — <id> is an 8-hex screenshot identifier.
//   2) A stream of UDP fragments (cmd 0x2000C0, i.e. FILEATTACHOPT + 0xC0) whose
//      body is:  "<id>|<totalSize>|<offset>|<fragCount>|<fragIndex>|<fragSize>|0|2|0|<mtime>#<data>"
//      Each fragment's <data> carries a single leading 0x00 before the image chunk.
// We collect the fragments, reassemble the JPEG, save it, and surface it to the
// frontend exactly like a normal received image (file.receive_request -> file.transfer_completed).

// FeiQ inline screenshot LZW decoder.
//
// Wire format: "LZW!"(4) + uint32 LE decoded DIB size(4) + uint32 LE CRC32(4) + LZW stream.
// The LZW stream uses FeiQ's own variant (NOT the GIF variant):
//   * No clear/end code. The dictionary is seeded with the 256 single-byte entries (0..255).
//   * Codes are bit-packed LSB-first WITHIN each code word (the sender stores each code
//     MSB-first in the byte stream then bit-reverses it, which is equivalent to reading the
//     chunk LSB-first). Bits are taken from the byte stream MSB-first (bit 7 of byte 0 first).
//   * Code width starts at 9 and grows with early change: when the code counter reaches
//     2^width it steps 9->10->11->12, capped at 12.
//   * New dictionary entry for code k is prev + entry[0]; the dictionary grows from 256 and
//     is frozen at 4096 entries (codes stay 12-bit afterwards).
//   * The decoded bytes are a BMP DIB (BITMAPINFOHEADER + pixel data), i.e. WITHOUT the
//     14-byte BITMAPFILEHEADER; the caller prepends it.
static bool LzwDecompress(const std::string& in, size_t inOff, size_t inLen,
                          int /*minCodeSize*/, size_t /*expectedOut*/, std::string& out) {
    out.clear();
    if (inOff + inLen > in.size()) return false;

    size_t bitPos = 0;
    auto readCode = [&](int codeSize) -> int {
        int code = 0;
        for (int i = 0; i < codeSize; ++i) {
            size_t byteIdx = inOff + (bitPos >> 3);
            if (byteIdx >= inOff + inLen) return -1;
            // MSB-first within the byte; assemble LSB-first into the code value.
            int bit = ((unsigned char)in[byteIdx] >> (7 - (bitPos & 7))) & 1;
            code |= bit << i;
            bitPos += 1;
        }
        return code;
    };

    int code = readCode(9);
    if (code < 0 || code > 255) return false;

    std::vector<std::string> dict;
    dict.reserve(4096);
    for (int i = 0; i < 256; ++i) dict.push_back(std::string(1, (char)i));

    int ds = 256;
    std::string prev(1, (char)code);
    out = prev;
    int codeSize = 9;
    // Counter matching FeiQ's encoder width-bump cadence (see growth below).
    int index = 257;

    while (true) {
        int k = readCode(codeSize);
        if (k < 0) break;                 // stream exhausted
        std::string entry;
        if (k < (int)dict.size()) {
            entry = dict[k];
        } else if (k == ds) {
            entry = prev + prev[0];       // KwKwK
        } else {
            return false;                 // corrupt stream / algorithm mismatch
        }
        out += entry;
        if (ds < 4096) {
            if ((int)dict.size() <= ds) dict.resize(ds + 1);
            dict[ds] = prev + entry[0];
            ++ds;
        }
        prev = entry;
        ++index;
        // FeiQ uses the "early change" convention: the code field widens one
        // step earlier than the raw dictionary size implies, so the bump lines
        // up with the encoder. Growing at ds == 2^codeSize (standard LZW) would
        // desync and corrupt screenshots, so we match the original counter.
        if ((1 << codeSize) == index && codeSize < 12) ++codeSize;
    }
    return !out.empty();
}

// LZW compressor that is the exact inverse of LzwDecompress above, so it
// produces a stream FeiQ (and our own decoder) can decompress. Key details
// that MUST match the decoder:
//   * codes are packed LSB-first into the bitstream; bytes are MSB-first.
//   * dictionary starts with 256 single-byte entries; next index = 256.
//   * code width starts at 9 and widens using the "early change" convention:
//     after each emitted code we do index++ and bump when (1<<codeSize)==index
//     (mirrors the decoder's per-iteration index++ / bump exactly).
static bool LzwCompress(const std::string& in, std::string& out) {
    out.clear();
    if (in.empty()) return false;
    std::vector<uint8_t> buf;
    uint32_t cur = 0;
    int nbits = 0;
    auto writeCode = [&](int code, int codeSize) {
        for (int i = 0; i < codeSize; ++i) {
            int bit = (code >> i) & 1;            // output code LSB first
            // Pack MSB-first into the byte stream so it matches the decoder's
            // readCode, which reads bit (7 - (bitPos&7)) i.e. MSB-first per byte.
            cur = (cur << 1) | (uint32_t)bit;
            ++nbits;
            if (nbits == 8) {
                buf.push_back((uint8_t)cur);
                cur = 0;
                nbits = 0;
            }
        }
    };

    // --- Must stay byte-for-byte symmetric with LzwDecompress ---
    // Decoder reads the FIRST code outside its loop: it does NOT allocate a
    // dictionary entry and does NOT bump the code width (its index starts at
    // 257 after that first code). Every subsequent code it reads allocates one
    // entry and bumps once. So the encoder must: emit the first code WITHOUT
    // bumping (with index already at 257), and bump exactly once per every
    // LATER code it emits. The previous version emitted an extra leading
    // literal + bump, which desynced the dictionary growth and truncated the
    // stream on decode.
    int codeSize = 9;
    int index = 257;                              // mirrors decoder right after code #1
    int ds = 256;
    std::unordered_map<std::string, int> dict;
    for (int i = 0; i < 256; ++i) dict[std::string(1, (char)i)] = i;

    auto bump = [&]() {
        ++index;
        if ((1 << codeSize) == index && codeSize < 12) ++codeSize;
    };

    bool first = true;                           // true until we emit the 1st code
    std::string w;                               // empty prefix to start
    for (size_t i = 0; i < in.size(); ++i) {
        char c = in[i];
        std::string wc = w + c;
        auto it = dict.find(wc);
        if (it != dict.end()) {
            w = wc;                              // extend the match; do not emit
        } else {
            writeCode(dict[w], codeSize);        // emit current prefix
            if (ds < 4096) { dict[wc] = ds; ++ds; }
            // The first emitted code pairs with the decoder's out-of-loop code,
            // which does NOT bump. All later codes bump once each.
            if (!first) bump();
            first = false;
            w = std::string(1, c);
        }
    }
    if (!w.empty()) {
        writeCode(dict[w], codeSize);            // final prefix
        // No later code follows, so bumping is irrelevant for the peer.
    }
    if (nbits > 0) buf.push_back((uint8_t)(cur << (8 - nbits)));  // flush
    out.assign((const char*)buf.data(), buf.size());
    return true;
}

bool CommandHandler::SendFeiQShotPayload(const UserInfo& target, const std::string& payload,
                                           int dw, int dh, uint32_t refCmd, uint32_t fragCmd) {
    if (refCmd == 0) refCmd = IPMSG_SENDMSG | IPMSG_SENDCHECKOPT | 0x1;   // 0x121
    if (fragCmd == 0) fragCmd = IPMSG_FILEATTACHOPT | 0xC0;              // 0x2000C0

    const size_t fragSize = 512;
    size_t total = payload.size();
    size_t fragCount = (total + fragSize - 1) / fragSize;
    if (fragCount == 0) fragCount = 1;

    // 8-hex screenshot id.
    static unsigned ssSeq = 0;
    uint32_t rnd = (uint32_t)((GetTickCount() ^
                               (std::hash<std::string>{}(target.Key())) ^
                               (++ssSeq * 2654435761u)) & 0xFFFFFFFF);
    char idBuf[16];
    snprintf(idBuf, sizeof(idBuf), "%08X", rnd);
    std::string id = idBuf;

    // Reference message: the inline-image marker "/~#><id>" matches the exact
    // format FeiQ itself uses. Dimensions mirror the real image so FeiQ sizes
    // the inline image correctly.
    std::string ref = "/~#>" + id + "<B~{/font;-11 0 0 0 " +
        std::to_string(dw) + " 0 0 0 " + std::to_string(dh) +
        " 0 0 2 32 微软雅黑 8404992;}";
    LogMessage("BRIDGE", "", "[FEIQ-SHOT-TX] Reference=\"" + ref + "\"");
    msgMng_->SendMessage(target, ref, refCmd);

    // DIAG dump of the verbatim payload for offline byte-diff.
    {
        std::ofstream o1(GetUserDownloadsDir() + "/FeiQ_OurTX_payload_" + id + ".bin", std::ios::binary);
        if (o1) o1.write(payload.data(), (std::streamsize)payload.size());
        LogMessage("BRIDGE", "", "[FEIQ-SHOT-TX] ref command=0x" +
            std::to_string(refCmd) + " (decimal " + std::to_string(refCmd) + ")");
    }

    char mtBuf[16];
    snprintf(mtBuf, sizeof(mtBuf), "%08X", 0);  // mtime always 0

    for (size_t i = 0; i < fragCount; ++i) {
        size_t off = i * fragSize;
        size_t len = (total - off < fragSize) ? (total - off) : fragSize;
        std::string chunk = payload.substr(off, len);

        std::string hdr;
        auto num = [&](size_t v) { hdr += std::to_string(v); hdr += '|'; };
        hdr += id; hdr += '|';
        num(total);
        num(off);
        num(fragCount);
        num(i + 1);
        num(len);
        hdr += "0|2|0|";
        hdr += mtBuf;
        hdr += '#';

        if (i == 0) {
            LogMessage("BRIDGE", "", "[FEIQ-SHOT-TX] Fragment header (ours)=" + hdr +
                "  [fields: id|total|off|fragCount|fragIndex|len|0|2|0|mtime]");
            LogMessage("BRIDGE", "", "[FEIQ-SHOT-TX] frag command=0x" +
                std::to_string(fragCmd) + " (decimal " + std::to_string(fragCmd) + ")");
        }

        // Fragment body = raw LZW payload immediately after '#'. The fragment
        // command MUST be exactly fragCmd (SendMessage would OR in 0x20).
        std::string body = hdr;
        body += chunk;

        if (i == 0) {
            std::ofstream of(GetUserDownloadsDir() + "/FeiQ_OurTX_frag0_" + id + ".bin", std::ios::binary);
            if (of) of.write(body.data(), (std::streamsize)body.size());
        }

        msgMng_->SendRawCommand(target, fragCmd, body);
    }
    return true;
}

nlohmann::json CommandHandler::HandleFeiQEchoScreenshot(const nlohmann::json& args) {
    nlohmann::json r;
    r["success"] = false;
    try {
        std::string payload, senderKey;
        uint32_t refCmd = 0, fragCmd = 0;
        int dw = 400, dh = 134;
        {
            std::lock_guard<std::mutex> lk(feiqMutex_);
            payload = lastFeiQShotPayload_;
            senderKey = lastFeiQShotSender_;
            refCmd = lastFeiQShotRefCmd_;
            fragCmd = lastFeiQShotFragCmd_;
            dw = lastFeiQShotW_;
            dh = lastFeiQShotH_;
        }
        if (payload.empty()) {
            r["error"] = "没有可回显的飞秋截图（请先收到一张飞秋发来的截图）";
            return r;
        }
        // Target: explicit arg wins; otherwise echo back to the original sender.
        nlohmann::json a = args;
        if ((!a.contains("target") || !a["target"].is_string() ||
             a["target"].get<std::string>().empty()) && !senderKey.empty()) {
            a["target"] = senderKey;
        }
        auto target = FindUserFromArgs(a);
        if (!target) { r["error"] = "未找到目标用户"; return r; }

        uint32_t useRef = refCmd ? refCmd : (IPMSG_SENDMSG | IPMSG_SENDCHECKOPT | 0x1);
        uint32_t useFrag = fragCmd ? fragCmd : (IPMSG_FILEATTACHOPT | 0xC0);
        LogMessage("BRIDGE", "", "[FEIQ-ECHO] ENTER target=" + target->Key() +
            " payload=" + std::to_string(payload.size()) +
            " refCmd=0x" + std::to_string(useRef) +
            " fragCmd=0x" + std::to_string(useFrag));

        SendFeiQShotPayload(*target, payload, dw, dh, useRef, useFrag);

        // DIAG: dump the echoed payload so it can be compared with the original
        // FeiQ_RawLZW_*.bin byte-for-byte.
        {
            static unsigned eSeq = 0;
            char buf[16]; snprintf(buf, sizeof(buf), "%08X", ++eSeq);
            std::ofstream o(GetUserDownloadsDir() + "/FeiQ_EchoTX_payload_" + std::string(buf) + ".bin", std::ios::binary);
            if (o) o.write(payload.data(), (std::streamsize)payload.size());
        }
        r["success"] = true;
        r["bytes"] = (int)payload.size();
    } catch (const std::exception& e) {
        r["error"] = e.what();
    }
    return r;
}

void CommandHandler::HandleFeiQScreenshotReference(const ipmsg::MsgBuf& msg, const std::string& body) {
    // body: "/~#><id><...>"
    size_t start = 4; // skip "/~#>"
    size_t end = body.find('<', start);
    if (end == std::string::npos) end = body.size();
    std::string id = body.substr(start, end - start);
    if (id.empty()) return;

    LogMessage("BRIDGE", "", "[FEIQ-SHOT-RX] Reference body=\"" + body + "\" id=" + id);
    LogMessage("BRIDGE", "", "[FEIQ-SHOT] Reference received id=" + id +
        " from=" + msg.sender.Key() + " command=0x" + std::to_string(msg.command));
    {
        std::lock_guard<std::mutex> lk(feiqMutex_);
        lastFeiQShotRefCmd_ = msg.command;   // remember FeiQ's own ref command
    }
    // NOTE: We deliberately do NOT emit file.receive_request here. FeiQ screenshots
    // are reassembled entirely on the backend, so there is no standard file transfer
    // for the frontend to "accept". Emitting it would create a stuck "waiting to
    // accept" bubble. The finished image is surfaced by FinalizeFeiQScreenshot.
    // The reassembly is keyed by "id|mtime" using the fragment payload (which also
    // carries the sender), so we don't need to persist a map entry here.
}

bool CommandHandler::HandleFeiQScreenshotFragment(const ipmsg::MsgBuf& msg) {
    const std::string& body = msg.body;
    size_t hash = body.find('#');
    if (hash == std::string::npos || hash == 0) return false;

    {
        std::lock_guard<std::mutex> lk(feiqMutex_);
        lastFeiQShotFragCmd_ = msg.command;  // remember FeiQ's own fragment command
    }

    // header must look like "<hexid>|<digits>|<digits>|<digits>|<digits>|<digits>|..."
    std::string header = body.substr(0, hash);
    if (header.empty() || !std::isxdigit((unsigned char)header[0])) return false;

    std::vector<std::string> f;
    size_t p = 0;
    while (true) {
        size_t q = header.find('|', p);
        if (q == std::string::npos) { f.push_back(header.substr(p)); break; }
        f.push_back(header.substr(p, q - p));
        p = q + 1;
    }
    if (f.size() < 6) return false;
    for (int i = 1; i <= 5; ++i) {
        if (f[i].empty()) return false;
        for (char c : f[i]) if (!std::isdigit((unsigned char)c)) return false;
    }

    auto ToInt = [](const std::string& s) {
        try { return std::stoi(s); } catch (...) { return 0; }
    };
    const std::string& id = f[0];
    int totalSize = ToInt(f[1]);
    int fragCount = ToInt(f[3]);
    int fragIndex = ToInt(f[4]);
    int fragSize  = ToInt(f[5]);
    (void)fragSize;
    std::string mtime = (f.size() >= 10) ? f[9] : "";  // observed always "00000000"
    // DIAG: log the real FeiQ fragment header (once per screenshot) so we can
    // compare its field layout against what we SEND ([FEIQ-SHOT-TX]).
    if (fragIndex == 1)
        LogMessage("BRIDGE", "", "[FEIQ-SHOT-RX] Fragment header (real FeiQ)=" + header);

    std::string data = body.substr(hash + 1);
    // Each fragment carries a single leading 0x00 before the image chunk.
    if (!data.empty() && (unsigned char)data[0] == 0x00) data.erase(0, 1);

    bool complete = false;
    {
        std::lock_guard<std::mutex> lk(feiqMutex_);
        // FeiQ has no reliable ack for inline screenshots and periodically
        // re-sends the whole fragment set (observed ~every 30s). If we've already
        // emitted this id, drop the fragment outright so we never reassemble /
        // re-emit the same image again.
        if (emittedIds_.count(id)) {
            LogMessage("BRIDGE", "", "[FEIQ-SHOT] Duplicate fragment ignored id=" + id +
                " (already emitted)");
            return true;
        }

        auto& shot = feiqShots_[id];
        if (shot.id.empty()) {
            shot.id = id;
            shot.sender = msg.sender;
            shot.senderKey = msg.sender.Key();
        }
        shot.totalSize = totalSize;
        shot.fragCount = fragCount;
        if (shot.frags.find(fragIndex) == shot.frags.end()) {
            shot.frags[fragIndex] = std::move(data);
        }
        int have = (int)shot.frags.size();
        // IMPORTANT: do NOT log every fragment. FeiQ bursts the entire fragment
        // set within tens of milliseconds (thousands of UDP packets); per-packet
        // synchronous disk logging blocks the receive thread and overflows the UDP
        // receive buffer, dropping fragments so the set can never be reassembled.
        // Only log at coarse progress steps.
        if (have % 100 == 0 || have == fragCount) {
            LogMessage("BRIDGE", "", "[FEIQ-SHOT] Progress id=" + id +
                " have=" + std::to_string(have) + "/" + std::to_string(fragCount));
        }
        if (fragCount > 0 && have >= fragCount) complete = true;
    }

    if (complete) FinalizeFeiQScreenshot(id);
    return true;
}

void CommandHandler::FinalizeFeiQScreenshot(const std::string& id) {
    FeiQScreenshot shot;
    {
        std::lock_guard<std::mutex> lk(feiqMutex_);
        auto it = feiqShots_.find(id);
        if (it == feiqShots_.end()) return;
        shot = std::move(it->second);
        feiqShots_.erase(it);
        // Permanently mark this id as emitted so any later re-send (FeiQ retries
        // the whole fragment set) is dropped by HandleFeiQScreenshotFragment.
        emittedIds_.insert(id);
    }

    // Reassemble fragments in fragIndex order (1-based)
    std::string buf;
    buf.reserve(shot.totalSize > 0 ? (size_t)shot.totalSize + 64 : 65536);
    int missing = 0;
    for (int i = 1; i <= shot.fragCount; ++i) {
        auto fit = shot.frags.find(i);
        if (fit == shot.frags.end()) { ++missing; continue; }
        buf += fit->second;
    }

    // Keep the verbatim FeiQ payload (the "LZW!" + size + crc + LZW(DIB) bytes we
    // just reassembled) so the UI can echo it back to the sender byte-for-byte.
    // NOTE: `buf` is later overwritten with a decoded BMP for display, so we must
    // snapshot it here.
    {
        std::lock_guard<std::mutex> lk(feiqMutex_);
        lastFeiQShotPayload_ = buf;
        lastFeiQShotSender_ = shot.senderKey;
    }

    // Diagnostics: log magic bytes so we can verify the decoded image format
    {
        std::ostringstream os;
        os << "[FEIQ-SHOT] Assembled id=" << id
           << " bytes=" << buf.size()
           << " expected=" << shot.totalSize
           << " missing=" << missing
           << " magic=";
        for (size_t i = 0; i < 8 && i < buf.size(); ++i)
            os << std::hex << (int)(unsigned char)buf[i] << " ";
        LogMessage("BRIDGE", "", os.str());
    }

    // Detect image format
    std::string ext = "jpg";
    static const unsigned char pngSig[8] = {0x89,0x50,0x4E,0x47,0x0D,0x0A,0x1A,0x0A};
    auto isJpeg = [&]() {
        return buf.size() >= 3 &&
               (unsigned char)buf[0] == 0xFF && (unsigned char)buf[1] == 0xD8 &&
               (unsigned char)buf[2] == 0xFF;
    };
    auto isPng = [&]() {
        return buf.size() >= 8 && std::memcmp(buf.data(), pngSig, 8) == 0;
    };

    // FeiQ LZW-compressed screenshots: "LZW!" + uint32 LE decoded DIB size + uint32 LE CRC
    // + LZW stream. The decoded stream is a BMP DIB (BITMAPINFOHEADER + pixels) WITHOUT the
    // 14-byte BITMAPFILEHEADER. We decode it and prepend the file header.
    auto le32 = [](const std::string& s, size_t o) -> uint32_t {
        return (uint32_t)(unsigned char)s[o]
             | ((uint32_t)(unsigned char)s[o + 1] << 8)
             | ((uint32_t)(unsigned char)s[o + 2] << 16)
             | ((uint32_t)(unsigned char)s[o + 3] << 24);
    };
    auto le16 = [](const std::string& s, size_t o) -> uint16_t {
        return (uint16_t)((unsigned char)s[o] | ((unsigned char)s[o + 1] << 8));
    };

    if (buf.size() >= 4 && buf[0] == 'L' && buf[1] == 'Z' && buf[2] == 'W' && buf[3] == '!') {
        size_t expectedOut = (buf.size() >= 8) ? (size_t)le32(buf, 4) : 0;
        uint32_t storedCrc = (buf.size() >= 12) ? le32(buf, 8) : 0;
        // DEBUG: keep the raw LZW payload for offline analysis.
        {
            std::ofstream rawF(GetUserDownloadsDir() + "/FeiQ_RawLZW_" + id + ".bin", std::ios::binary);
            if (rawF) rawF.write(buf.data(), (std::streamsize)buf.size());
        }
        std::string dib;
        bool ok = LzwDecompress(buf, 12, buf.size() - 12, 8, expectedOut, dib);
        bool validDib = ok && dib.size() >= 40;
        if (validDib) {
            uint32_t biSize = le32(dib, 0);
            int32_t biWidth = (int32_t)le32(dib, 4);
            int32_t biHeight = (int32_t)le32(dib, 8);
            uint16_t biBitCount = le16(dib, 14);
            LogMessage("BRIDGE", "", "[FEIQ-SHOT-RX] DIB w=" + std::to_string(biWidth) +
                " h=" + std::to_string(biHeight) + " bitcount=" + std::to_string(biBitCount) +
                " sizeImage=" + std::to_string(le32(dib, 20)));
            if (biSize < 40 || biSize > 256 || biWidth <= 0 || biHeight == 0 ||
                (biBitCount != 24 && biBitCount != 32)) {
                validDib = false;
            } else {
                int bytesPerRow = ((biWidth * biBitCount + 31) / 32) * 4;
                if (dib.size() > expectedOut && expectedOut >= (size_t)biSize) {
                    // FeiQ's LZW stream decodes to a coherent DIB followed by trailing
                    // bits that expand into extra (ignored) pixel data. Trim to the
                    // exact size declared in the packet header so we emit a clean BMP.
                    dib.resize(expectedOut);
                    LogMessage("BRIDGE", "", "[FEIQ-SHOT] Trimmed decoded DIB " +
                        std::to_string(dib.size()) + " -> declared " +
                        std::to_string(expectedOut) + " bytes");
                } else if (dib.size() < expectedOut) {
                    // Decoded DIB is shorter than the header claims (e.g. a dropped
                    // fragment): clamp biHeight to the rows we actually have so the
                    // BMP is still valid and renders the available portion.
                    int64_t avail = (int64_t)dib.size() - (int64_t)biSize;
                    if (avail < 0) avail = 0;
                    int32_t availRows = (int32_t)(avail / bytesPerRow);
                    if (availRows > 0 && availRows < biHeight) {
                        biHeight = availRows;
                        dib[8]  = (char)(biHeight & 0xFF);
                        dib[9]  = (char)((biHeight >> 8) & 0xFF);
                        dib[10] = (char)((biHeight >> 16) & 0xFF);
                        dib[11] = (char)((biHeight >> 24) & 0xFF);
                        LogMessage("BRIDGE", "", "[FEIQ-SHOT] Decoded DIB truncated; clamped biHeight to " +
                            std::to_string(availRows) + " of declared rows");
                    }
                }
                // Build full BMP with BITMAPFILEHEADER.
                std::string bmp;
                bmp.reserve(dib.size() + 14);
                uint32_t bfOffBits = 14 + biSize;
                uint32_t bfSize = 14 + (uint32_t)dib.size();
                bmp.push_back('B'); bmp.push_back('M');
                bmp.push_back((char)(bfSize & 0xFF));
                bmp.push_back((char)((bfSize >> 8) & 0xFF));
                bmp.push_back((char)((bfSize >> 16) & 0xFF));
                bmp.push_back((char)((bfSize >> 24) & 0xFF));
                bmp.push_back((char)0); bmp.push_back((char)0); // reserved1
                bmp.push_back((char)0); bmp.push_back((char)0); // reserved2
                bmp.push_back((char)(bfOffBits & 0xFF));
                bmp.push_back((char)((bfOffBits >> 8) & 0xFF));
                bmp.push_back((char)((bfOffBits >> 16) & 0xFF));
                bmp.push_back((char)((bfOffBits >> 24) & 0xFF));
                bmp += dib;
                buf = std::move(bmp);
                ext = "bmp";
                uint32_t actualCrc = Crc32(dib);
                // DIAG: dump the DIB header fields that affect the pixel start
                // offset, plus the first pixel bytes. For a solid-color image the
                // first row should be the solid color (not garbage); mismatch
                // here pinpoints where the parsing goes wrong (header vs palette
                // vs row stride). Also keep the raw decoded DIB for offline view.
                {
                    std::ofstream dibF(GetUserDownloadsDir() + "/FeiQ_DecodedDIB_" + id + ".bin", std::ios::binary);
                    if (dibF) dibF.write(dib.data(), (std::streamsize)dib.size());
                }
                uint32_t biCompression = le32(dib, 16);
                uint32_t biSizeImage    = le32(dib, 20);
                uint32_t biClrUsed      = le32(dib, 32);
                std::ostringstream px;
                px << "[FEIQ-SHOT] DIB head: biSize=" << biSize
                   << " biCompression=" << biCompression
                   << " biClrUsed=" << biClrUsed
                   << " biSizeImage=" << biSizeImage
                   << " pixelStart=" << biSize << " firstPixels=";
                size_t px0 = (size_t)biSize;
                for (size_t i = 0; i + px0 < dib.size() && i < 32; ++i)
                    px << std::hex << (int)(unsigned char)dib[px0 + i] << " ";
                LogMessage("BRIDGE", "", px.str());
                LogMessage("BRIDGE", "", "[FEIQ-SHOT] LZW decoded BMP: DIB=" +
                    std::to_string(dib.size()) + " expected=" + std::to_string(expectedOut) +
                    " crc=" + std::to_string(actualCrc) +
                    (actualCrc == storedCrc ? " (MATCH)" : " (mismatch stored=0x" +
                        std::to_string(storedCrc) + ")") +
                    " biWidth=" + std::to_string(biWidth) +
                    " biHeight=" + std::to_string(biHeight) +
                    " bpp=" + std::to_string(biBitCount));
                {
                    std::lock_guard<std::mutex> lk(feiqMutex_);
                    lastFeiQShotW_ = biWidth;
                    lastFeiQShotH_ = biHeight;
                }
            }
        }
        if (!validDib) {
            LogMessage("BRIDGE", "", std::string("[FEIQ-SHOT] LZW decompress failed; saving raw as .bin") +
                (ok ? " (invalid DIB header)" : " (decode error)"));
            ext = "bin";
        }
    } else if (isJpeg()) {
        ext = "jpg";
    } else if (isPng()) {
        ext = "png";
    } else {
        // Some residual offset: search for JPEG SOI within the first 256 bytes
        bool found = false;
        size_t limit = (std::min)((size_t)256, buf.size());
        for (size_t i = 0; i + 3 <= limit; ++i) {
            if ((unsigned char)buf[i] == 0xFF && (unsigned char)buf[i+1] == 0xD8 &&
                (unsigned char)buf[i+2] == 0xFF) {
                buf = buf.substr(i);
                ext = "jpg";
                found = true;
                break;
            }
        }
        if (!found) {
            ext = "bin"; // unknown - save raw for inspection
            LogMessage("BRIDGE", "", "[FEIQ-SHOT] Unknown image magic; saving raw as .bin");
        }
    }

    // Save to Downloads\IPMsgPro. Build via std::filesystem::path so the separator
    // is normalized to the OS-native style (backslashes on Windows), avoiding
    // mixed "/" + "\" paths that some APIs dislike.
    std::filesystem::path dirPath = std::filesystem::u8path(GetUserDownloadsDir()) / "IPMsgPro";
    std::error_code ec;
    std::filesystem::create_directories(dirPath, ec);
    std::filesystem::path savePath = dirPath / ("FeiQ_Screenshot_" + id + "." + ext);
    std::string savePathStr = savePath.u8string();

    {
        std::ofstream out(savePath, std::ios::binary);
        if (!out) {
            LogMessage("BRIDGE", "", "[FEIQ-SHOT] Failed to open output: " + savePathStr);
            return;
        }
        out.write(buf.data(), (std::streamsize)buf.size());
    }

    LogMessage("BRIDGE", "", "[FEIQ-SHOT] Saved " + savePathStr + " (" +
        std::to_string(buf.size()) + " bytes)");

    // Surface the reassembled image directly to the frontend as a finished image
    // message (inline base64 data URL), bypassing the "accept file" UI entirely.
    std::string mime = (ext == "png") ? "png" : (ext == "jpg" ? "jpeg" : "octet-stream");
    std::string dataUrl = "data:image/" + mime + ";base64," + Base64Encode(buf);

    bridge_->Emit("feiq.screenshot_received", {
        {"fromUser", UserToJson(shot.sender)},
        {"dataUrl", dataUrl},
        {"fileName", "\xe9\xa3\x9e\xe7\xa7\x8b\xe6\x88\xaa\xe5\x9b\xbe_" + id + "." + ext},
        {"savePath", savePathStr},
        {"fileSize", (int64_t)buf.size()}
    });
}

} // namespace ipmsg
