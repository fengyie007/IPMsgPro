// ============================================================================
// Message Database Implementation (SQLite3)
// ============================================================================

#include "message_db.h"
#include "sqlite3.h"
#include <sstream>

namespace ipmsg {

MessageDB::MessageDB() = default;

MessageDB::~MessageDB() {
    Close();
}

bool MessageDB::Init(const std::string& dbPath) {
    // If already initialized with the same path, do nothing
    if (db_ && dbPath_ == dbPath) return true;

    // Close existing connection if any (different path or fresh init)
    Close();

    dbPath_ = dbPath;
    int rc = sqlite3_open(dbPath.c_str(), &db_);
    if (rc != SQLITE_OK) {
        sqlite3_close(db_);
        db_ = nullptr;
        dbPath_.clear();
        return false;
    }

    // Enable WAL mode for better concurrent read performance
    sqlite3_exec(db_, "PRAGMA journal_mode=WAL;", nullptr, nullptr, nullptr);
    sqlite3_exec(db_, "PRAGMA foreign_keys=ON;", nullptr, nullptr, nullptr);

    if (!CreateTables()) {
        sqlite3_close(db_);
        db_ = nullptr;
        return false;
    }

    return true;
}

void MessageDB::Close() {
    if (db_) {
        sqlite3_close(db_);
        db_ = nullptr;
    }
}

bool MessageDB::CreateTables() {
    const char* sql = R"(
        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            from_id TEXT NOT NULL,
            to_id TEXT NOT NULL,
            content TEXT NOT NULL DEFAULT '',
            type INTEGER NOT NULL DEFAULT 0,
            timestamp INTEGER NOT NULL DEFAULT 0,
            status INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_messages_from ON messages(from_id);
        CREATE INDEX IF NOT EXISTS idx_messages_to ON messages(to_id);
        CREATE INDEX IF NOT EXISTS idx_messages_timestamp ON messages(timestamp);
        CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(from_id, to_id, timestamp);
    )";

    char* errMsg = nullptr;
    int rc = sqlite3_exec(db_, sql, nullptr, nullptr, &errMsg);
    if (rc != SQLITE_OK) {
        sqlite3_free(errMsg);
        return false;
    }

    return true;
}

bool MessageDB::SaveMessage(const MessageRecord& msg) {
    if (!db_) return false;

    const char* sql = R"(
        INSERT OR IGNORE INTO messages (id, from_id, to_id, content, type, timestamp, status)
        VALUES (?, ?, ?, ?, ?, ?, ?)
    )";

    sqlite3_stmt* stmt = nullptr;
    if (sqlite3_prepare_v2(db_, sql, -1, &stmt, nullptr) != SQLITE_OK) {
        return false;
    }

    sqlite3_bind_text(stmt, 1, msg.id.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(stmt, 2, msg.fromId.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(stmt, 3, msg.toId.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(stmt, 4, msg.content.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_int(stmt, 5, msg.type);
    sqlite3_bind_int64(stmt, 6, msg.timestamp);
    sqlite3_bind_int(stmt, 7, msg.status);

    int rc = sqlite3_step(stmt);
    sqlite3_finalize(stmt);

    return rc == SQLITE_DONE;
}

bool MessageDB::GetMessages(const std::string& userId, const std::string& localUserId,
                             int limit, int offset,
                             std::vector<MessageRecord>& messages) {
    if (!db_) return false;

    // Get messages between current user and the other user
    // Messages where:
    // - from_id = localUserId AND to_id = userId (sent by me to you)
    // - from_id = userId AND to_id = localUserId (sent by you to me)
    // ORDER BY ASC so oldest messages appear first (chat display order)
    const char* sql = R"(
        SELECT id, from_id, to_id, content, type, timestamp, status
        FROM messages
        WHERE (from_id = ? AND to_id = ?) OR (from_id = ? AND to_id = ?)
        ORDER BY timestamp ASC
        LIMIT ? OFFSET ?
    )";

    sqlite3_stmt* stmt = nullptr;
    if (sqlite3_prepare_v2(db_, sql, -1, &stmt, nullptr) != SQLITE_OK) {
        return false;
    }

    sqlite3_bind_text(stmt, 1, localUserId.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(stmt, 2, userId.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(stmt, 3, userId.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(stmt, 4, localUserId.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_int(stmt, 5, limit);
    sqlite3_bind_int(stmt, 6, offset);

    while (sqlite3_step(stmt) == SQLITE_ROW) {
        MessageRecord msg;
        msg.id = reinterpret_cast<const char*>(sqlite3_column_text(stmt, 0));
        msg.fromId = reinterpret_cast<const char*>(sqlite3_column_text(stmt, 1));
        msg.toId = reinterpret_cast<const char*>(sqlite3_column_text(stmt, 2));
        msg.content = reinterpret_cast<const char*>(sqlite3_column_text(stmt, 3));
        msg.type = sqlite3_column_int(stmt, 4);
        msg.timestamp = sqlite3_column_int64(stmt, 5);
        msg.status = sqlite3_column_int(stmt, 6);
        messages.push_back(std::move(msg));
    }

    sqlite3_finalize(stmt);
    return true;
}

bool MessageDB::SearchMessages(const std::string& keyword,
                                std::vector<MessageRecord>& messages) {
    if (!db_ || keyword.empty()) return false;

    const char* sql = R"(
        SELECT id, from_id, to_id, content, type, timestamp, status
        FROM messages
        WHERE content LIKE ?
        ORDER BY timestamp DESC
        LIMIT 100
    )";

    sqlite3_stmt* stmt = nullptr;
    if (sqlite3_prepare_v2(db_, sql, -1, &stmt, nullptr) != SQLITE_OK) {
        return false;
    }

    std::string pattern = "%" + keyword + "%";
    sqlite3_bind_text(stmt, 1, pattern.c_str(), -1, SQLITE_TRANSIENT);

    while (sqlite3_step(stmt) == SQLITE_ROW) {
        MessageRecord msg;
        msg.id = reinterpret_cast<const char*>(sqlite3_column_text(stmt, 0));
        msg.fromId = reinterpret_cast<const char*>(sqlite3_column_text(stmt, 1));
        msg.toId = reinterpret_cast<const char*>(sqlite3_column_text(stmt, 2));
        msg.content = reinterpret_cast<const char*>(sqlite3_column_text(stmt, 3));
        msg.type = sqlite3_column_int(stmt, 4);
        msg.timestamp = sqlite3_column_int64(stmt, 5);
        msg.status = sqlite3_column_int(stmt, 6);
        messages.push_back(std::move(msg));
    }

    sqlite3_finalize(stmt);
    return true;
}

bool MessageDB::ClearMessages(const std::string& userId) {
    if (!db_) return false;

    if (userId.empty()) {
        sqlite3_exec(db_, "DELETE FROM messages;", nullptr, nullptr, nullptr);
    } else {
        const char* sql = "DELETE FROM messages WHERE from_id = ? OR to_id = ?;";
        sqlite3_stmt* stmt = nullptr;
        if (sqlite3_prepare_v2(db_, sql, -1, &stmt, nullptr) != SQLITE_OK) {
            return false;
        }
        sqlite3_bind_text(stmt, 1, userId.c_str(), -1, SQLITE_TRANSIENT);
        sqlite3_bind_text(stmt, 2, userId.c_str(), -1, SQLITE_TRANSIENT);
        sqlite3_step(stmt);
        sqlite3_finalize(stmt);
    }

    return true;
}

int MessageDB::GetMessageCount(const std::string& userId) {
    if (!db_) return 0;

    const char* sql = "SELECT COUNT(*) FROM messages WHERE from_id = ? OR to_id = ?;";
    sqlite3_stmt* stmt = nullptr;
    if (sqlite3_prepare_v2(db_, sql, -1, &stmt, nullptr) != SQLITE_OK) {
        return 0;
    }

    sqlite3_bind_text(stmt, 1, userId.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(stmt, 2, userId.c_str(), -1, SQLITE_TRANSIENT);

    int count = 0;
    if (sqlite3_step(stmt) == SQLITE_ROW) {
        count = sqlite3_column_int(stmt, 0);
    }

    sqlite3_finalize(stmt);
    return count;
}

bool MessageDB::GetRecentConversations(const std::string& localUserId, int limit, std::vector<MessageRecord>& messages) {
    if (!db_) return false;

    // Get latest message per conversation partner
    // We need to find the "other party" for each conversation
    const char* sql = R"(
        SELECT m.id, m.from_id, m.to_id, m.content, m.type, m.timestamp, m.status
        FROM messages m
        INNER JOIN (
            SELECT 
                CASE 
                    WHEN from_id = ? THEN to_id 
                    ELSE from_id 
                END as partner_id,
                MAX(timestamp) as max_ts
            FROM messages
            GROUP BY partner_id
        ) latest ON (
            (m.from_id = ? AND m.to_id = latest.partner_id) OR 
            (m.from_id = latest.partner_id AND m.to_id = ?)
        ) AND m.timestamp = latest.max_ts
        ORDER BY m.timestamp DESC
        LIMIT ?
    )";

    sqlite3_stmt* stmt = nullptr;
    if (sqlite3_prepare_v2(db_, sql, -1, &stmt, nullptr) != SQLITE_OK) {
        return false;
    }

    sqlite3_bind_text(stmt, 1, localUserId.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(stmt, 2, localUserId.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_text(stmt, 3, localUserId.c_str(), -1, SQLITE_TRANSIENT);
    sqlite3_bind_int(stmt, 4, limit);

    while (sqlite3_step(stmt) == SQLITE_ROW) {
        MessageRecord msg;
        msg.id = reinterpret_cast<const char*>(sqlite3_column_text(stmt, 0));
        msg.fromId = reinterpret_cast<const char*>(sqlite3_column_text(stmt, 1));
        msg.toId = reinterpret_cast<const char*>(sqlite3_column_text(stmt, 2));
        msg.content = reinterpret_cast<const char*>(sqlite3_column_text(stmt, 3));
        msg.type = sqlite3_column_int(stmt, 4);
        msg.timestamp = sqlite3_column_int64(stmt, 5);
        msg.status = sqlite3_column_int(stmt, 6);
        messages.push_back(std::move(msg));
    }

    sqlite3_finalize(stmt);
    return true;
}

} // namespace ipmsg
