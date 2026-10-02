#pragma once
// ============================================================================
// FeiQ (飞秋) inline screenshot receive path
// ----------------------------------------------------------------------------
// FeiQ does NOT send screenshots as a standard IPMsg FILEATTACH. Instead:
//   1) A reference message (cmd SENDMSG|SENDCHECKOPT) whose body begins with
//      "/~#><id><...>" — <id> is an 8-hex screenshot identifier.
//   2) A burst of UDP fragments (cmd 0x2000C0 = FILEATTACHOPT + 0xC0) whose
//      body is "<id>|<totalSize>|<offset>|<fragCount>|<fragIndex>|<fragSize>|0|2|0|<mtime>#<data>",
//      each <data> carrying a single leading 0x00 before the image chunk.
// The assembled payload is either a raw JPEG/PNG or FeiQ's "LZW!" + DIB
// container, which is decoded into a BMP.
//
// Thread-safety: HandleReference/HandleFragment run on the UDP receive thread;
// the internal state is guarded by a mutex.
// ============================================================================

#include "ipmsg/msgmng.h"

#include <cstdint>
#include <functional>
#include <map>
#include <mutex>
#include <optional>
#include <set>
#include <string>

namespace ipmsg {

/// A fully reassembled screenshot that has already been written to disk.
struct FeiQScreenshotResult {
    UserInfo sender;
    std::string id;        // FeiQ's 8-hex screenshot id
    std::string bytes;     // image file bytes (BMP / JPEG / PNG, or raw on failure)
    std::string ext;       // "bmp" / "jpg" / "png" / "bin"
    std::string savePath;  // UTF-8 path under Downloads\IPMsgPro
};

class FeiQScreenshotAssembler {
public:
    /// Optional sink for protocol diagnostics (raw LZW payload, decoded DIB).
    using DebugDump = std::function<void(const std::string& fileName, const std::string& data)>;

    static bool IsReference(const std::string& body);
    static bool IsFragment(uint32_t command, const std::string& body);

    void SetDebugDump(DebugDump dump);
    using FragmentAck = std::function<void(const UserInfo&, const std::string&, int)>;
    void SetFragmentAck(FragmentAck ack) { ack_ = std::move(ack); }

    /// Log the reference message. Nothing is surfaced to the UI yet: the
    /// finished image arrives through HandleFragment.
    void HandleReference(const MsgBuf& msg);

    /// Consume one fragment. Returns false when the body is not a valid
    /// fragment header (the caller should treat the message normally). When
    /// the last fragment arrives, the image is decoded, saved, and `result`
    /// is filled. Fragments of an id that was already delivered are dropped
    /// (FeiQ re-sends the whole set periodically).
    bool HandleFragment(const MsgBuf& msg, std::optional<FeiQScreenshotResult>& result);

private:
    struct Shot {
        std::string id;
        std::string senderKey;
        int totalSize = 0;
        int fragCount = 0;
        time_t updated = 0;
        UserInfo sender;                    // captured from the first fragment
        std::map<int, std::string> frags;   // fragIndex -> chunk bytes (leading 0x00 stripped)
    };

    std::optional<FeiQScreenshotResult> Finalize(const std::string& id);
    void Dump(const std::string& fileName, const std::string& data) const;

    std::map<std::string, Shot> shots_;
    // Ids already reassembled + delivered. FeiQ has no reliable ack for inline
    // screenshots and re-sends the fragment set (~every 30s) until it gives
    // up; anything in this set is dropped so an image is never delivered
    // twice. Kept for the session (ids are random 8-hex; volume is tiny).
    std::set<std::string> emittedIds_;
    std::set<std::string> rejectedIds_; // never ACK a retry of a failed assembly
    std::mutex mutex_;
    DebugDump dump_;
    FragmentAck ack_;
};

}  // namespace ipmsg
