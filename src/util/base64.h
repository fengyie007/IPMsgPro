#pragma once
// ============================================================================
// Base64 helpers (single definition for the whole backend)
// ============================================================================

#include <string>

namespace ipmsg {

/// Standard Base64 with '=' padding.
std::string Base64Encode(const std::string& in);

/// Inverse of Base64Encode. Stops at the first '=' or invalid character;
/// returns an empty string for empty/invalid input.
std::string Base64Decode(const std::string& in);

}  // namespace ipmsg
