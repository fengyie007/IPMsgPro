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

#include <algorithm>
#include <cctype>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <vector>

namespace ipmsg {

namespace {

// CRC32 (IEEE 802.3) for diagnostics / DIB integrity checks.
uint32_t Crc32(const std::string& data) {
    uint32_t crc = 0xFFFFFFFF;
    for (unsigned char c : data) {
        crc ^= c;
        for (int i = 0; i < 8; ++i)
            crc = (crc & 1) ? (0xEDB88320 ^ (crc >> 1)) : (crc >> 1);
    }
    return crc ^ 0xFFFFFFFF;
}

}  // namespace

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
//   * New dictionary entry for code k is prev + entry[0]; after entry 4095,
//     overwrite entries cyclically from 256 (codes stay 12-bit, no clear code).
//   * The decoded bytes are a BMP DIB (BITMAPINFOHEADER + pixel data), i.e. WITHOUT the
//     14-byte BITMAPFILEHEADER; the caller prepends it.
static bool LzwDecompress(const std::string& in, size_t inOff, size_t inLen,
                          int /*minCodeSize*/, size_t expectedOut, std::string& out) {
    out.clear();
    if (inOff > in.size() || inLen > in.size() - inOff || expectedOut == 0) return false;

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
        if (k == ds) {
            // KwKwK also applies after wraparound: dict[ds] is then stale.
            entry = prev + prev[0];
        } else if (k < (int)dict.size()) {
            entry = dict[k];
        } else {
            return false;                 // corrupt stream / algorithm mismatch
        }
        if (entry.size() > expectedOut - out.size()) return false;
        out += entry;
        if ((int)dict.size() <= ds) dict.resize(ds + 1);
        dict[ds] = prev + entry[0];
        if (++ds == 4096) ds = 256;
        prev = entry;
        // Width stops growing at 12; keep the counter bounded for large images.
        if (codeSize < 12) ++index;
        // FeiQ uses the "early change" convention: the code field widens one
        // step earlier than the raw dictionary size implies, so the bump lines
        // up with the encoder. Growing at ds == 2^codeSize (standard LZW) would
        // desync and corrupt screenshots, so we match the original counter.
        if ((1 << codeSize) == index && codeSize < 12) ++codeSize;
    }
    return out.size() == expectedOut;
}

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
    const std::string& body = msg.body;
    size_t hash = body.find('#');
    if (hash == std::string::npos || hash == 0) return false;

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
        LogMessage("FEIQ", "", "[FEIQ-SHOT-RX] Fragment header (real FeiQ)=" + header);

    std::string data = body.substr(hash + 1);
    // Each fragment carries a single leading 0x00 before the image chunk.
    if (!data.empty() && (unsigned char)data[0] == 0x00) data.erase(0, 1);

    bool complete = false;
    {
        std::lock_guard<std::mutex> lk(mutex_);
        // FeiQ has no reliable ack for inline screenshots and periodically
        // re-sends the whole fragment set (observed ~every 30s). If we've already
        // emitted this id, drop the fragment outright so we never reassemble /
        // re-emit the same image again.
        if (emittedIds_.count(id)) {
            LogMessage("FEIQ", "", "[FEIQ-SHOT] Duplicate fragment ignored id=" + id +
                " (already emitted)");
            return true;
        }

        auto& shot = shots_[id];
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
            LogMessage("FEIQ", "", "[FEIQ-SHOT] Progress id=" + id +
                " have=" + std::to_string(have) + "/" + std::to_string(fragCount));
        }
        if (fragCount > 0 && have >= fragCount) complete = true;
    }

    if (complete) result = Finalize(id);
    return true;
}

std::optional<FeiQScreenshotResult> FeiQScreenshotAssembler::Finalize(const std::string& id) {
    Shot shot;
    {
        std::lock_guard<std::mutex> lk(mutex_);
        auto it = shots_.find(id);
        if (it == shots_.end()) return std::nullopt;
        shot = std::move(it->second);
        shots_.erase(it);
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
        bool ok = LzwDecompress(buf, 12, buf.size() - 12, 8, expectedOut, dib);
        bool validDib = ok && dib.size() >= 40 && Crc32(dib) == storedCrc;
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
                uint32_t actualCrc = Crc32(dib);
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
    std::string savePathStr = enc::WideToUtf8(savePath.wstring());

    {
        std::ofstream out(savePath, std::ios::binary);
        if (!out) {
            LogMessage("FEIQ", "", "[FEIQ-SHOT] Failed to open output: " + savePathStr);
            return std::nullopt;
        }
        out.write(buf.data(), (std::streamsize)buf.size());
    }

    LogMessage("FEIQ", "", "[FEIQ-SHOT] Saved " + savePathStr + " (" +
        std::to_string(buf.size()) + " bytes)");

    // Keep undecodable payloads for diagnostics, but never emit them as images.
    if (ext == "bin") return std::nullopt;

    FeiQScreenshotResult out;
    out.sender = shot.sender;
    out.id = id;
    out.bytes = std::move(buf);
    out.ext = ext;
    out.savePath = savePathStr;
    return out;
}

}  // namespace ipmsg
