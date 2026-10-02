#include "feiq_lzw.h"
#include <unordered_map>
#include <vector>

namespace ipmsg::feiq {

uint32_t Crc32(const std::string& data) {
    uint32_t crc = 0xFFFFFFFF;
    for (unsigned char c : data) {
        crc ^= c;
        for (int i = 0; i < 8; ++i)
            crc = (crc & 1) ? (0xEDB88320 ^ (crc >> 1)) : (crc >> 1);
    }
    return crc ^ 0xFFFFFFFF;
}

// FeiQ uses no clear/end code. Code bits are LSB-first, stream bytes MSB-first.
// Width grows with the emitted-code counter, not the cycling dictionary index.
// At 4096 entries slots 256..4095 are overwritten; width remains 12 bits.
bool LzwCompress(const std::string& in, std::string& out) {
    out.clear();
    if (in.empty() || in.size() > kMaxDibBytes) return false;
    std::unordered_map<std::string, int> dict;
    std::vector<std::string> slots(4096);
    for (int i = 0; i < 256; ++i) {
        slots[i] = std::string(1, static_cast<char>(i));
        dict[slots[i]] = i;
    }
    int next = 256, width = 9, counter = 256, bits = 0;
    unsigned byte = 0;
    auto emit = [&](int code) {
        for (int i = 0; i < width; ++i) {
            byte = (byte << 1) | ((code >> i) & 1);
            if (++bits == 8) {
                out.push_back(static_cast<char>(byte));
                byte = 0;
                bits = 0;
            }
        }
        if (width < 12 && ++counter == (1 << width)) ++width;
    };
    std::string prefix(1, in[0]);
    for (size_t i = 1; i < in.size(); ++i) {
        std::string candidate = prefix + in[i];
        auto it = dict.find(candidate);
        if (it != dict.end()) {
            prefix = std::move(candidate);
            continue;
        }
        emit(dict.at(prefix));
        if (out.size() > kMaxPayloadBytes - 12) return false;
        // Remove the reverse mapping before recycling a slot, or the encoder
        // could emit an index whose old string no longer exists at the receiver.
        if (!slots[next].empty()) {
            auto old = dict.find(slots[next]);
            if (old != dict.end() && old->second == next) dict.erase(old);
        }
        slots[next] = candidate;
        dict[std::move(candidate)] = next;
        if (++next == 4096) next = 256;
        prefix.assign(1, in[i]);
    }
    emit(dict.at(prefix));
    if (bits) out.push_back(static_cast<char>(byte << (8 - bits)));
    return out.size() <= kMaxPayloadBytes - 12;
}

bool LzwDecompress(const std::string& in, size_t inOff, size_t inLen,
                   size_t expectedOut, std::string& out) {
    out.clear();
    if (inOff > in.size() || inLen > in.size() - inOff ||
        expectedOut == 0 || expectedOut > kMaxDibBytes) return false;
    size_t bitPos = 0;
    auto readCode = [&](int codeSize) -> int {
        int code = 0;
        for (int i = 0; i < codeSize; ++i) {
            size_t byteIdx = inOff + (bitPos >> 3);
            if (byteIdx >= inOff + inLen) return -1;
            int bit = (static_cast<unsigned char>(in[byteIdx]) >> (7 - (bitPos & 7))) & 1;
            code |= bit << i;
            ++bitPos;
        }
        return code;
    };
    int code = readCode(9);
    if (code < 0 || code > 255) return false;
    std::vector<std::string> dict;
    dict.reserve(4096);
    for (int i = 0; i < 256; ++i) dict.emplace_back(1, static_cast<char>(i));
    int next = 256, width = 9, counter = 257;
    std::string prev(1, static_cast<char>(code));
    out = prev;
    while (true) {
        int k = readCode(width);
        if (k < 0) break;
        std::string entry;
        if (k == next) entry = prev + prev[0]; // KwKwK, including recycled slots
        else if (k < static_cast<int>(dict.size())) entry = dict[k];
        else return false;
        if (entry.size() > expectedOut - out.size()) return false;
        out += entry;
        if (static_cast<int>(dict.size()) <= next) dict.resize(next + 1);
        dict[next] = prev + entry[0];
        if (++next == 4096) next = 256;
        prev = std::move(entry);
        if (width < 12 && ++counter == (1 << width)) ++width;
    }
    return out.size() == expectedOut;
}

} // namespace ipmsg::feiq
