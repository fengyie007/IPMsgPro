// ============================================================================
// FeiQ (飞秋) inline screenshot receive path
// ----------------------------------------------------------------------------
// See feiq_screenshot.h for the wire protocol. This file collects the UDP
// fragments, reassembles and decodes the image, saves it under
// Downloads\IPMsgPro and hands the result back to the bridge layer.
// ============================================================================
#include "bridge/feiq_screenshot.h"
#include "logger.h"
#include "util/app_paths.h"
#include "util/encoding.h"
#include "util/feiq_lzw.h"

#include <algorithm>
#include <cctype>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <vector>

namespace ipmsg {

bool FeiQScreenshotAssembler::IsReference(const std::string& body) {
    return body.size() >= 4 && body.compare(0, 4, "/~#>") == 0;
}

bool FeiQScreenshotAssembler::IsFragment(uint32_t command, const std::string& body) {
    return (command & IPMSG_FILEATTACHOPT) != 0 && !body.empty() &&
           body.find('|') != std::string::npos &&
           body.find('#') != std::string::npos &&
           std::isxdigit(static_cast<unsigned char>(body[0]));
}

void FeiQScreenshotAssembler::SetDebugDump(DebugDump dump) {
    dump_ = std::move(dump);
}

void FeiQScreenshotAssembler::Dump(const std::string& fileName, const std::string& data) const {
    if (dump_) dump_(fileName, data);
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

void FeiQScreenshotAssembler::HandleReference(const MsgBuf& msg) {
    const std::string& body = msg.body;
    // body: "/~#><id><...>"
    size_t start = 4; // skip "/~#>"
    size_t end = body.find('<', start);
    if (end == std::string::npos) end = body.size();
    std::string id = body.substr(start, end - start);
    if (id.empty()) return;

    LogMessage("FEIQ", "DEBUG", "[FEIQ-SHOT-RX] Reference body=\"" + body + "\" id=" + id);
    LogMessage("FEIQ", "", "[FEIQ-SHOT] Reference received id=" + id +
        " from=" + msg.sender.Key() + " command=0x" + std::to_string(msg.command));
    // NOTE: We deliberately do NOT emit file.receive_request here. FeiQ screenshots
    // are reassembled entirely on the backend, so there is no standard file transfer
    // for the frontend to "accept". Emitting it would create a stuck "waiting to
    // accept" bubble. The finished image is surfaced by FinalizeFeiQScreenshot.
    // The reassembly is keyed by "id|mtime" using the fragment payload (which also
    // carries the sender), so we don't need to persist a map entry here.
}

bool FeiQScreenshotAssembler::HandleFragment(const MsgBuf& msg, std::optional<FeiQScreenshotResult>& result) {
    result.reset();
    const std::string& body = msg.body;
    size_t hash = body.find('#');
    if (hash == std::string::npos || hash == 0 || hash > 256) return false;

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
    if (f.size() != 10 || f[0].size() != 8) return true;
    for (unsigned char c : f[0]) if (!std::isxdigit(c)) return true;
    for (int i = 1; i <= 5; ++i) {
        if (f[i].empty() || f[i].size() > 9) return false;
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
    int offset = ToInt(f[2]);
    if (totalSize <= 0 || totalSize > static_cast<int>(feiq::kMaxPayloadBytes) ||
        fragCount != (totalSize + 511) / 512 || fragIndex < 1 || fragIndex > fragCount ||
        offset != (fragIndex - 1) * 512 || fragSize != (std::min)(512, totalSize - offset) ||
        body.size() != hash + 2 + static_cast<size_t>(fragSize) || body[hash + 1] != '\0') {
        LogMessage("FEIQ", "WARN", "[FEIQ-SHOT] Invalid fragment geometry id=" + id);
        return true;
    }
    // DIAG: log the real FeiQ fragment header (once per screenshot) so we can
    // compare its field layout against what we SEND ([FEIQ-SHOT-TX]).
    if (fragIndex == 1)
        LogMessage("FEIQ", "", "[FEIQ-SHOT-RX] Fragment header (real FeiQ)=" + header);

    const std::string data = body.substr(hash + 2);
    const std::string key = msg.sender.Key() + ":" + msg.sender.ipAddress + ":" +
        std::to_string(msg.sender.portNo) + ":" + id;
    bool complete = false;
    {
        std::lock_guard<std::mutex> lk(mutex_);
        const time_t now = std::time(nullptr);
        for (auto it = shots_.begin(); it != shots_.end();) {
            if (now - it->second.updated > 120) it = shots_.erase(it);
            else ++it;
        }
        if (rejectedIds_.count(key)) return true;
        if (!emittedIds_.count(key)) {
            if (!shots_.count(key) && shots_.size() >= 8) return true;
            auto& shot = shots_[key];
            if (shot.id.empty()) {
                shot.id = id;
                shot.sender = msg.sender;
                shot.senderKey = msg.sender.Key();
                shot.totalSize = totalSize;
                shot.fragCount = fragCount;
            }
            if (shot.totalSize != totalSize || shot.fragCount != fragCount) return true;
            auto it = shot.frags.find(fragIndex);
            if (it != shot.frags.end() && it->second != data) return true;
            shot.frags.emplace(fragIndex, data);
            shot.updated = now;
            complete = shot.frags.size() == static_cast<size_t>(fragCount);
        }
    }
    if (complete) {
        // Mark as rejected first so exceptions or failed validation cannot turn
        // a later retry of the final fragment into a false successful ACK.
        {
            std::lock_guard<std::mutex> lk(mutex_);
            rejectedIds_.insert(key);
        }
        result = Finalize(key);
        if (result) {
            std::lock_guard<std::mutex> lk(mutex_);
            rejectedIds_.erase(key);
        }
    }
    // ACK valid duplicates as well; the sender may have lost our previous ACK.
    // Withhold the final ACK if the assembled image is corrupt.
    if (ack_ && (!complete || result)) ack_(msg.sender, id, fragIndex);
    return true;
}

std::optional<FeiQScreenshotResult> FeiQScreenshotAssembler::Finalize(const std::string& key) {
    Shot shot;
    {
        std::lock_guard<std::mutex> lk(mutex_);
        auto it = shots_.find(key);
        if (it == shots_.end()) return std::nullopt;
        shot = std::move(it->second);
        shots_.erase(it);
    }
    const std::string& id = shot.id;

    // Reassemble fragments in fragIndex order (1-based)
    std::string buf;
    buf.reserve(shot.totalSize > 0 ? (size_t)shot.totalSize + 64 : 65536);
    int missing = 0;
    for (int i = 1; i <= shot.fragCount; ++i) {
        auto fit = shot.frags.find(i);
        if (fit == shot.frags.end()) { ++missing; continue; }
        buf += fit->second;
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
        LogMessage("FEIQ", "", os.str());
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
        // DEBUG only: keep the raw LZW payload for offline analysis.
        Dump("FeiQ_RawLZW_" + id + ".bin", buf);
        std::string dib;
        bool ok = feiq::LzwDecompress(buf, 12, buf.size() - 12, expectedOut, dib);
        bool validDib = ok && dib.size() >= 40 && feiq::Crc32(dib) == storedCrc;
        if (ok && !validDib) {
            LogMessage("FEIQ", "WARN", "[FEIQ-SHOT] Decoded DIB failed integrity check id=" + id);
        }
        if (validDib) {
            uint32_t biSize = le32(dib, 0);
            int32_t biWidth = (int32_t)le32(dib, 4);
            int32_t biHeight = (int32_t)le32(dib, 8);
            uint16_t biBitCount = le16(dib, 14);
            LogMessage("FEIQ", "DEBUG", "[FEIQ-SHOT-RX] DIB w=" + std::to_string(biWidth) +
                " h=" + std::to_string(biHeight) + " bitcount=" + std::to_string(biBitCount) +
                " sizeImage=" + std::to_string(le32(dib, 20)));
            if (biSize < 40 || biSize > 256 || biWidth <= 0 || biHeight == 0 ||
                (biBitCount != 24 && biBitCount != 32)) {
                validDib = false;
            } else {
                // Length and CRC were checked before interpreting the DIB. Never
                // crop or pad a corrupt decode to make it appear to be a valid image.
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
                uint32_t actualCrc = feiq::Crc32(dib);
                // DIAG (DEBUG only): dump the DIB header fields that affect the
                // pixel start offset, plus the first pixel bytes. For a solid-color
                // image the first row should be the solid color (not garbage);
                // mismatch here pinpoints where the parsing goes wrong (header vs
                // palette vs row stride). Also keep the raw decoded DIB.
                Dump("FeiQ_DecodedDIB_" + id + ".bin", dib);
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
                LogMessage("FEIQ", "DEBUG", px.str());
                LogMessage("FEIQ", "", "[FEIQ-SHOT] LZW decoded BMP: DIB=" +
                    std::to_string(dib.size()) + " expected=" + std::to_string(expectedOut) +
                    " crc=" + std::to_string(actualCrc) +
                    (actualCrc == storedCrc ? " (MATCH)" : " (mismatch stored=0x" +
                        std::to_string(storedCrc) + ")") +
                    " biWidth=" + std::to_string(biWidth) +
                    " biHeight=" + std::to_string(biHeight) +
                    " bpp=" + std::to_string(biBitCount));
            }
        }
        if (!validDib) {
            LogMessage("FEIQ", "", std::string("[FEIQ-SHOT] LZW decompress failed; saving raw as .bin") +
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
            LogMessage("FEIQ", "", "[FEIQ-SHOT] Unknown image magic; saving raw as .bin");
        }
    }

    // Save to Downloads\IPMsgPro. Build via std::filesystem::path so the separator
    // is normalized to the OS-native style (backslashes on Windows), avoiding
    // mixed "/" + "\" paths that some APIs dislike.
    std::filesystem::path dirPath = enc::PathFromUtf8(paths::UserDownloadsDir()) / "IPMsgPro";
    std::error_code ec;
    std::filesystem::create_directories(dirPath, ec);
    std::filesystem::path savePath = dirPath / ("FeiQ_Screenshot_" + id + "." + ext);
    for (unsigned suffix = 1; std::filesystem::exists(savePath); ++suffix) {
        if (suffix > 10000) return std::nullopt;
        savePath = dirPath / ("FeiQ_Screenshot_" + id + "_" + std::to_string(suffix) + "." + ext);
    }
    std::string savePathStr = enc::WideToUtf8(savePath.wstring());

    {
        std::ofstream out(savePath, std::ios::binary);
        if (!out) {
            LogMessage("FEIQ", "", "[FEIQ-SHOT] Failed to open output: " + savePathStr);
            return std::nullopt;
        }
        out.write(buf.data(), (std::streamsize)buf.size());
        out.close();
        if (!out) {
            LogMessage("FEIQ", "ERROR", "[FEIQ-SHOT] Failed to write output: " + savePathStr);
            return std::nullopt;
        }
    }

    LogMessage("FEIQ", "", "[FEIQ-SHOT] Saved " + savePathStr + " (" +
        std::to_string(buf.size()) + " bytes)");

    // Keep undecodable payloads for diagnostics, but never emit them as images.
    if (ext == "bin") return std::nullopt;
    {
        std::lock_guard<std::mutex> lk(mutex_);
        emittedIds_.insert(key);
    }

    FeiQScreenshotResult out;
    out.sender = shot.sender;
    out.id = id;
    out.bytes = std::move(buf);
    out.ext = ext;
    out.savePath = savePathStr;
    return out;
}

}  // namespace ipmsg
