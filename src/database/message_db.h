#pragma once
// ============================================================================
// Message Database (SQLite3)
// Manages persistent storage of chat history
// ============================================================================

#include <string>
#include <vector>
#include <cstdint>

struct sqlite3;

namespace ipmsg {

/// Message record stored in the database
struct MessageRecord {
    std::string id;         // unique message ID
    std::string fromId;     // sender key (userName@hostName)
    std::string toId;       // receiver key
    std::string content;    // message text content
    int type = 0;           // 0:text, 1:image, 2:file
    int64_t timestamp = 0;  // unix timestamp
    int status = 0;         // 0:sending, 1:delivered, 2:read, 3:failed
};

/// SQLite3-backed message database
class MessageDB {
public:
    MessageDB();
    ~MessageDB();

    // Non-copyable
    MessageDB(const MessageDB&) = delete;
    MessageDB& operator=(const MessageDB&) = delete;

    /// Initialize database at the given path
    /// Creates tables if they don't exist
    bool Init(const std::string& dbPath);

    /// Close the database
    void Close();

    /// Check if database is ready
    bool IsReady() const { return db_ != nullptr; }

    /// Save a message to the database
    bool SaveMessage(const MessageRecord& msg);

    /// Get messages for a specific user (conversation partner)
    /// userId is the key of the other party (userName@hostName)
    /// localUserId is the current user's key
    /// Returns messages ordered by timestamp ASC (oldest first)
    bool GetMessages(const std::string& userId, const std::string& localUserId,
                     int limit, int offset,
                     std::vector<MessageRecord>& messages);

    /// Search messages by keyword
    bool SearchMessages(const std::string& keyword,
                        std::vector<MessageRecord>& messages);

    /// Clear messages for a specific user, or all if userId is empty
    bool ClearMessages(const std::string& userId = "");

    /// Get total message count for a user
    int GetMessageCount(const std::string& userId);

    /// Get recent conversations - latest message per user for conversation list
    /// localUserId is the current user's key to determine conversation partners
    /// Returns up to limit conversations, sorted by latest message timestamp DESC
    bool GetRecentConversations(const std::string& localUserId, int limit, std::vector<MessageRecord>& messages);

private:
    /// Create database tables
    bool CreateTables();

    sqlite3* db_ = nullptr;
    std::string dbPath_;
};

} // namespace ipmsg
