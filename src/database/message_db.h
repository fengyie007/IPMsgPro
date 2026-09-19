#pragma once
// ============================================================================
// Message Database (SQLite3)
// Manages persistent storage of chat history
// ============================================================================

#include <string>
#include <vector>
#include <cstdint>
#include <mutex>

struct sqlite3;

namespace ipmsg {

/// Message status codes persisted in the `status` column.
/// Text messages: Sending (no ack yet) -> Delivered (RECVMSG received) or Failed.
/// File/image messages: Sending (transfer in progress) -> Completed or Failed.
/// Incoming messages are stored as Delivered.
enum MessageStatus : int {
    kMsgStatusSending   = 0,
    kMsgStatusDelivered = 1,
    kMsgStatusCompleted = 2,  // file transfer finished successfully
    kMsgStatusFailed    = 3,
};

/// Message record stored in the database
struct MessageRecord {
    std::string id;         // unique message ID
    std::string fromId;     // sender key (userName@hostName)
    std::string toId;       // receiver key
    std::string content;    // message text content
    int type = 0;           // 0:text, 1:image, 2:file
    int64_t timestamp = 0;  // unix timestamp
    int status = kMsgStatusSending;  // see MessageStatus
};

/// SQLite3-backed message database.
/// All public methods are thread-safe: the UI thread (commands), the UDP
/// receive thread (incoming messages) and the TCP transfer threads (status
/// updates) share one instance.
class MessageDB {
public:
    MessageDB();
    ~MessageDB();

    // Non-copyable
    MessageDB(const MessageDB&) = delete;
    MessageDB& operator=(const MessageDB&) = delete;

    /// Initialize database at the given path (UTF-8).
    /// Creates tables if they don't exist. Reopening with a different path
    /// closes the current connection first, atomically with respect to the
    /// other methods, so callers must not call Close() beforehand.
    bool Init(const std::string& dbPath);

    /// Close the database
    void Close();

    /// Check if database is ready
    bool IsReady() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return db_ != nullptr;
    }

    /// Save a message to the database
    bool SaveMessage(const MessageRecord& msg);

    /// Update the status of an existing message (see MessageStatus).
    /// Returns false when the database is closed or the id does not exist.
    bool UpdateStatus(const std::string& id, int status);

    /// Get messages for a specific user (conversation partner)
    /// userId is the key of the other party (userName@hostName)
    /// localUserId is the current user's key
    /// Returns the NEWEST `limit` messages after skipping `offset` newer ones
    /// (offset pages backwards in time), ordered by timestamp ASC (oldest first)
    bool GetMessages(const std::string& userId, const std::string& localUserId,
                     int limit, int offset,
                     std::vector<MessageRecord>& messages);

    /// Search messages by keyword
    bool SearchMessages(const std::string& keyword,
                        std::vector<MessageRecord>& messages);

    /// Clear messages for a specific user, or all if userId is empty
    bool ClearMessages(const std::string& userId = "");

    /// Get recent conversations - latest message per user for conversation list
    /// localUserId is the current user's key to determine conversation partners
    /// Returns up to limit conversations, sorted by latest message timestamp DESC
    bool GetRecentConversations(const std::string& localUserId, int limit, std::vector<MessageRecord>& messages);

private:
    /// Create database tables (caller holds mutex_)
    bool CreateTables();

    /// Close the connection (caller holds mutex_)
    void CloseLocked();

    // Serializes every call on this object. SQLite itself is compiled in
    // serialized mode, but Init()/Close() swap the connection pointer while
    // another thread may be inside SaveMessage()/UpdateStatus().
    mutable std::mutex mutex_;
    sqlite3* db_ = nullptr;
    std::string dbPath_;
};

} // namespace ipmsg
