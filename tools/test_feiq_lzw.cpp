// Standalone codec regression test (no GUI or network). Build from a VS prompt:
// cl /std:c++17 /EHsc /utf-8 tools/test_feiq_lzw.cpp src/util/feiq_lzw.cpp /Fe:feiq_lzw_test.exe
// Optional real sample: feiq_lzw_test.exe <FeiQ_RawLZW_ID.bin> <sender-ID.bmp>
#include "../src/util/feiq_lzw.h"
#include <cassert>
#include <fstream>
#include <iostream>
#include <iterator>
#include <random>
#include <stdexcept>
#include <vector>

static void Check(bool ok, const char* text) {
    if (!ok) throw std::runtime_error(text);
}
static uint32_t Read32(const std::string& s, size_t at) {
    uint32_t n = 0;
    for (int i = 0; i < 4; ++i) n |= uint32_t(static_cast<unsigned char>(s.at(at + i))) << (i * 8);
    return n;
}
static std::string Read(const char* path) {
    std::ifstream file(path, std::ios::binary);
    if (!file) throw std::runtime_error("Cannot open sample");
    return {std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()};
}
// Independent reference decoder: read a bit-string then interpret codes using a
// map of whole strings. It is intentionally not linked to production decoding.
static std::string Reference(const std::string& encoded) {
    std::string bits;
    for (unsigned char byte : encoded)
        for (int b = 7; b >= 0; --b) bits.push_back((byte >> b) & 1 ? '1' : '0');
    std::vector<std::string> dictionary(4096);
    for (int i = 0; i < 256; ++i) dictionary[i] = std::string(1, char(i));
    std::string result, prev;
    size_t pos = 0;
    int width = 9, counter = 256, next = 256;
    while (pos + width <= bits.size()) {
        int code = 0;
        for (int i = 0; i < width; ++i) if (bits[pos++] == '1') code |= 1 << i;
        std::string entry = !prev.empty() && code == next ? prev + prev[0] : dictionary[code];
        Check(!entry.empty(), "Reference decoded an undefined code");
        result += entry;
        if (!prev.empty()) {
            dictionary[next] = prev + entry[0];
            if (++next == 4096) next = 256;
        }
        prev = entry;
        if (width < 12 && ++counter == (1 << width)) ++width;
    }
    return result;
}
static void RoundTrip(const std::string& raw) {
    std::string encoded, decoded;
    Check(ipmsg::feiq::LzwCompress(raw, encoded), "Encode failed");
    Check(Reference(encoded) == raw, "Independent reference mismatch");
    Check(ipmsg::feiq::LzwDecompress(encoded, 0, encoded.size(), raw.size(), decoded), "Decode failed");
    Check(decoded == raw, "Round trip mismatch");
    Check(!ipmsg::feiq::LzwDecompress(encoded, 0, encoded.size(), raw.size() + 1, decoded), "Accepted wrong size");
    Check(!ipmsg::feiq::LzwDecompress(encoded, encoded.size() + 1, 1, raw.size(), decoded), "Accepted invalid offset");
}
int main(int argc, char** argv) {
    try {
        Check(ipmsg::feiq::Crc32("123456789") == 0xcbf43926u, "CRC32 mismatch");
        std::string out;
        Check(!ipmsg::feiq::LzwCompress("", out), "Accepted empty source");
        Check(!ipmsg::feiq::LzwDecompress("", 12, size_t(-1), 40, out), "Accepted truncated header");
        RoundTrip("A"); RoundTrip("ABC"); RoundTrip("ABABABA");
        RoundTrip(std::string(200000, '\0'));
        RoundTrip(std::string(200000, 'A'));
        std::mt19937 random(20261001);
        for (size_t n : {255, 256, 257, 512, 1024, 2048, 4096, 50000, 1000000}) {
            std::string data(n, '\0');
            for (char& c : data) c = static_cast<char>(random() & 255);
            RoundTrip(data);
        }
        if (argc == 3) {
            const auto sample = Read(argv[1]), bmp = Read(argv[2]);
            Check(sample.size() >= 12 && sample.compare(0, 4, "LZW!") == 0, "Invalid raw fixture");
            Check(bmp.size() >= 54 && bmp.substr(0, 2) == "BM", "Invalid BMP fixture");
            const auto expected = bmp.substr(14);
            Check(ipmsg::feiq::LzwDecompress(sample, 12, sample.size() - 12, Read32(sample, 4), out), "Real decode failed");
            Check(out == expected && ipmsg::feiq::Crc32(out) == Read32(sample, 8), "Real sample mismatch");
            Check(Reference(sample.substr(12)) == expected, "Reference cannot decode FeiQ fixture");
            RoundTrip(expected);
        }
        std::cout << "All FeiQ codec tests passed\n";
        return 0;
    } catch (const std::exception& e) {
        std::cerr << e.what() << '\n';
        return 1;
    }
}
