#pragma once
#include "ipmsg/msgmng.h"
#include "database/message_db.h"
#include <condition_variable>
#include <deque>
#include <memory>
#include <set>

namespace ipmsg {

// Owns a bounded queue and one joinable worker. No image conversion or ACK wait
// runs on the WebView/UDP thread. Stop before destroying MsgMng or MessageDB.
class FeiQImageSender {
public:
    struct Task {
        UserInfo target;
        std::string localId, messageId, imageId, sourcePath, filePath, fileName;
        uint64_t fileSize = 0;
        bool copied = false;
    };
    using EventCallback = std::function<void(const Task&, const std::string&, int, const std::string&)>;
    ~FeiQImageSender();
    void Init(MsgMng& messages, MessageDB& database, EventCallback callback);
    bool Enqueue(const UserInfo& target, const std::string& source, const std::string& dataDir,
                 Task& task, std::string& error);
    void HandleAck(const MsgBuf& message);
    void Shutdown();

private:
    struct Job {
        Task task;
        uint64_t referencePacket = 0;
        bool referenceAck = false;
        std::vector<bool> acked, sent;
        size_t acknowledged = 0;
    };
    void Run();
    void Send(const std::shared_ptr<Job>& job);
    void Publish(const Task& task, const std::string& state, int progress, const std::string& error = "");
    bool WaitStopped(int milliseconds);
    MsgMng* messages_ = nullptr;
    MessageDB* database_ = nullptr;
    EventCallback callback_;
    std::mutex mutex_;
    std::condition_variable changed_;
    std::thread worker_;
    bool stopping_ = false;
    std::deque<std::shared_ptr<Job>> queue_;
    std::shared_ptr<Job> active_;
    std::set<std::string> imageIds_;
};

} // namespace ipmsg
