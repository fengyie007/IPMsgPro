// ============================================================================
// Base64 helpers implementation
// ============================================================================
#include "util/base64.h"

#include <cctype>
#include <cstdint>

namespace ipmsg {

std::string Base64Encode(const std::string& in) {
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
std::string Base64Decode(const std::string& in) {
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

}  // namespace ipmsg
