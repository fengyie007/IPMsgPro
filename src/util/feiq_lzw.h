#pragma once
#include <cstddef>
#include <cstdint>
#include <string>

namespace ipmsg::feiq {

constexpr size_t kMaxImageBytes = 20 * 1024 * 1024;
constexpr size_t kMaxDibBytes = 64 * 1024 * 1024;
constexpr size_t kMaxPayloadBytes = 16 * 1024 * 1024;
constexpr size_t kFragmentBytes = 512;

uint32_t Crc32(const std::string& data);
bool LzwCompress(const std::string& in, std::string& out);
bool LzwDecompress(const std::string& in, size_t offset, size_t length,
                   size_t expectedOut, std::string& out);

} // namespace ipmsg::feiq
