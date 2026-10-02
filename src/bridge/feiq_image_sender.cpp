#include "feiq_image_sender.h"
#include "util/encoding.h"
#include "util/feiq_lzw.h"
#include "logger.h"
#include <windows.h>
#include <objidl.h>
#include <gdiplus.h>
#include <wrl/client.h>
#include <algorithm>
#include <charconv>
#include <chrono>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <random>
#include <sstream>
#include <stdexcept>

namespace ipmsg {
namespace {
namespace fs = std::filesystem;

void Append32(std::string& bytes, uint32_t value) {
    for (int i = 0; i < 4; ++i) bytes.push_back(static_cast<char>(value >> (i * 8)));
}

struct GdiSession {
    ULONG_PTR token = 0;
    GdiSession() {
        Gdiplus::GdiplusStartupInput input;
        if (Gdiplus::GdiplusStartup(&token, &input, nullptr) != Gdiplus::Ok)
            throw std::runtime_error("无法初始化图片解码器");
    }
    ~GdiSession() { Gdiplus::GdiplusShutdown(token); }
};

// Rasterize PNG/JPEG/BMP to the same bottom-up 24-bit DIB FeiQ sends us.
std::string ToDib(const std::string& bytes) {
    GdiSession session;
    Microsoft::WRL::ComPtr<IStream> stream;
    if (FAILED(CreateStreamOnHGlobal(nullptr, TRUE, &stream)))
        throw std::runtime_error("无法读取图片");
    ULONG written = 0;
    if (FAILED(stream->Write(bytes.data(), static_cast<ULONG>(bytes.size()), &written)) || written != bytes.size())
        throw std::runtime_error("无法读取图片数据");
    LARGE_INTEGER zero = {};
    if (FAILED(stream->Seek(zero, STREAM_SEEK_SET, nullptr))) throw std::runtime_error("无法读取图片流");
    std::unique_ptr<Gdiplus::Bitmap> image(Gdiplus::Bitmap::FromStream(stream.Get(), FALSE));
    if (!image || image->GetLastStatus() != Gdiplus::Ok) throw std::runtime_error("无效的图片文件");
    GUID format = {};
    image->GetRawFormat(&format);
    if (!IsEqualGUID(format, Gdiplus::ImageFormatPNG) && !IsEqualGUID(format, Gdiplus::ImageFormatJPEG) &&
        !IsEqualGUID(format, Gdiplus::ImageFormatBMP)) throw std::runtime_error("仅支持 PNG、JPEG、BMP 图片");
    const uint64_t width = image->GetWidth(), height = image->GetHeight();
    const uint64_t stride = (width * 3 + 3) & ~uint64_t(3);
    if (!width || !height || width > 16384 || height > 16384 || width * height > 16000000 ||
        stride * height + 40 > feiq::kMaxDibBytes) throw std::runtime_error("图片像素过大，请改用文件发送");
    Gdiplus::Bitmap rgb(static_cast<INT>(width), static_cast<INT>(height), PixelFormat24bppRGB);
    {
        Gdiplus::Graphics graphics(&rgb);
        if (graphics.Clear(Gdiplus::Color::White) != Gdiplus::Ok ||
            graphics.DrawImage(image.get(), 0, 0, static_cast<INT>(width), static_cast<INT>(height)) != Gdiplus::Ok)
            throw std::runtime_error("图片转换失败");
    }
    std::string dib;
    Append32(dib, 40); Append32(dib, static_cast<uint32_t>(width)); Append32(dib, static_cast<uint32_t>(height));
    dib.append("\x01\x00\x18\x00", 4); // planes=1, bitcount=24
    Append32(dib, 0); Append32(dib, static_cast<uint32_t>(stride * height));
    for (int i = 0; i < 4; ++i) Append32(dib, 0);
    dib.resize(40 + static_cast<size_t>(stride * height), '\0');
    Gdiplus::Rect rect(0, 0, static_cast<INT>(width), static_cast<INT>(height));
    Gdiplus::BitmapData pixels = {};
    if (rgb.LockBits(&rect, Gdiplus::ImageLockModeRead, PixelFormat24bppRGB, &pixels) != Gdiplus::Ok)
        throw std::runtime_error("无法读取图片像素");
    for (size_t y = 0; y < height; ++y) {
        const auto* row = static_cast<const char*>(pixels.Scan0) + static_cast<ptrdiff_t>(y) * pixels.Stride;
        std::memcpy(&dib[40 + (height - 1 - y) * stride], row, static_cast<size_t>(width * 3));
    }
    rgb.UnlockBits(&pixels);
    return dib;
}

std::string ReadImage(const std::string& path) {
    std::ifstream input(enc::PathFromUtf8(path), std::ios::binary | std::ios::ate);
    if (!input) throw std::runtime_error("无法打开图片文件");
    const auto size = input.tellg();
    if (size <= 0 || size > static_cast<std::streamoff>(feiq::kMaxImageBytes))
        throw std::runtime_error("图片为空或超过 20 MB，请改用文件发送");
    std::string bytes(static_cast<size_t>(size), '\0');
    input.seekg(0);
    if (!input.read(bytes.data(), static_cast<std::streamsize>(bytes.size())))
        throw std::runtime_error("读取图片失败");
    return bytes;
}
} // namespace

FeiQImageSender::~FeiQImageSender() { Shutdown(); }

void FeiQImageSender::Init(MsgMng& messages, MessageDB& database, EventCallback callback) {
    messages_ = &messages;
    database_ = &database;
    callback_ = std::move(callback);
}

bool FeiQImageSender::Enqueue(const UserInfo& target, const std::string& source,
                             const std::string& dataDir, Task& task, std::string& error) {
    try {
        std::lock_guard<std::mutex> lock(mutex_);
        if (stopping_ || !messages_ || !messages_->IsReady()) throw std::runtime_error("网络尚未就绪");
        if (queue_.size() + (active_ ? 1 : 0) >= 4) throw std::runtime_error("图片发送队列已满，请稍后重试");
        auto sourcePath = enc::PathFromUtf8(source);
        if (!fs::is_regular_file(sourcePath)) throw std::runtime_error("图片文件不存在");
        task.fileSize = fs::file_size(sourcePath);
        if (!task.fileSize || task.fileSize > feiq::kMaxImageBytes) throw std::runtime_error("图片为空或超过 20 MB");
        std::random_device random;
        do {
            std::ostringstream id;
            id << std::hex << std::setfill('0') << std::setw(8) << static_cast<uint32_t>(random());
            task.imageId = id.str();
        } while (imageIds_.count(task.imageId));
        imageIds_.insert(task.imageId);
        const auto now = std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::system_clock::now().time_since_epoch()).count();
        task.messageId = "image_" + std::to_string(now) + "_" + task.imageId;
        task.target = target;
        task.localId = messages_->GetLocalUser().Key();
        task.sourcePath = source;
        task.fileName = enc::WideToUtf8(sourcePath.filename().wstring());
        // A unique per-message directory retains the original extension for history previews.
        task.filePath = enc::WideToUtf8((enc::PathFromUtf8(dataDir) / "images" /
            task.messageId / sourcePath.filename()).wstring());
        if (!worker_.joinable()) worker_ = std::thread(&FeiQImageSender::Run, this);
        auto job = std::make_shared<Job>();
        job->task = task;
        queue_.push_back(std::move(job));
        changed_.notify_all();
        return true;
    } catch (const std::exception& e) {
        error = e.what();
        return false;
    }
}

void FeiQImageSender::HandleAck(const MsgBuf& message) {
    std::lock_guard<std::mutex> lock(mutex_);
    if (stopping_ || !active_) return;
    auto& job = *active_;
    const auto& peer = job.task.target;
    if (message.sender.Key() != peer.Key() || message.sender.ipAddress != peer.ipAddress ||
        message.sender.portNo != peer.portNo) return;
    const uint32_t mode = GET_MODE(message.command);
    if (mode == IPMSG_RECVMSG) {
        uint64_t packet = 0;
        const auto parsed = std::from_chars(message.body.data(), message.body.data() + message.body.size(), packet);
        if (parsed.ec == std::errc{} && parsed.ptr == message.body.data() + message.body.size() &&
            packet && packet == job.referencePacket) job.referenceAck = true;
    } else if (mode == IPMSG_REPORT_RECVIMAGE) {
        const std::string prefix = job.task.imageId + "|";
        if (message.body.compare(0, prefix.size(), prefix) != 0 || message.body.back() != '#') return;
        unsigned index = 0;
        const char* end = message.body.data() + message.body.size() - 1;
        const auto parsed = std::from_chars(message.body.data() + prefix.size(), end, index);
        if (parsed.ec != std::errc{} || parsed.ptr != end || index == 0 || index > job.acked.size()) return;
        if (job.sent[index - 1] && !job.acked[index - 1]) {
            job.acked[index - 1] = true;
            ++job.acknowledged;
        }
    }
    changed_.notify_all();
}

bool FeiQImageSender::WaitStopped(int milliseconds) {
    std::unique_lock<std::mutex> lock(mutex_);
    return changed_.wait_for(lock, std::chrono::milliseconds(milliseconds), [&] { return stopping_; });
}

void FeiQImageSender::Shutdown() {
    {
        std::lock_guard<std::mutex> lock(mutex_);
        stopping_ = true;
        changed_.notify_all();
    }
    if (worker_.joinable()) worker_.join();
}

void FeiQImageSender::Publish(const Task& task, const std::string& state, int progress, const std::string& error) {
    // Callbacks must never escape the worker (e.g. the WebView is shutting down).
    try { if (callback_) callback_(task, state, progress, error); }
    catch (const std::exception& e) { LogMessage("IMAGE", "ERROR", e.what()); }
    catch (...) { LogMessage("IMAGE", "ERROR", "Image event callback failed"); }
}

void FeiQImageSender::Run() {
    while (true) {
        std::shared_ptr<Job> job;
        bool stop = false;
        {
            std::unique_lock<std::mutex> lock(mutex_);
            changed_.wait(lock, [&] { return stopping_ || !queue_.empty(); });
            if (queue_.empty()) return;
            job = queue_.front(); queue_.pop_front(); active_ = job;
            stop = stopping_;
        }
        std::string error;
        try {
            if (stop) throw std::runtime_error("程序退出，图片发送已取消");
            Send(job);
        } catch (const std::exception& e) { error = e.what(); }
        catch (...) { error = "图片发送发生未知错误"; }
        if (!error.empty()) {
            try { database_->UpdateStatus(job->task.messageId, kMsgStatusFailed); } catch (...) {}
            Publish(job->task, "failed", 0, error);
            LogMessage("IMAGE", "WARN", "Send failed id=" + job->task.imageId + ": " + error);
        }
        {
            std::lock_guard<std::mutex> lock(mutex_);
            active_.reset();
        }
    }
}

void FeiQImageSender::Send(const std::shared_ptr<Job>& job) {
    auto& task = job->task;
    const std::string image = ReadImage(task.sourcePath);
    const std::string dib = ToDib(image);
    if (WaitStopped(0)) throw std::runtime_error("图片发送已取消");
    std::string compressed;
    if (!feiq::LzwCompress(dib, compressed)) throw std::runtime_error("压缩图片过大，请改用文件发送");
    // Catch encoder regressions before sending an undecodable image to a peer.
    std::string verified;
    if (!feiq::LzwDecompress(compressed, 0, compressed.size(), dib.size(), verified) || verified != dib)
        throw std::runtime_error("图片压缩校验失败");
    std::string payload = "LZW!";
    Append32(payload, static_cast<uint32_t>(dib.size())); Append32(payload, feiq::Crc32(dib));
    payload += compressed;
    const auto path = enc::PathFromUtf8(task.filePath);
    fs::create_directories(path.parent_path());
    {
        std::ofstream output(path, std::ios::binary);
        output.write(image.data(), static_cast<std::streamsize>(image.size()));
        output.close();
        if (!output) throw std::runtime_error("无法保存发送图片副本");
    }
    task.copied = true;
    task.fileSize = image.size();
    MessageRecord record;
    record.id = task.messageId; record.fromId = task.localId; record.toId = task.target.Key();
    record.content = task.filePath; record.type = 1; record.timestamp = std::time(nullptr);
    record.status = kMsgStatusSending;
    if (!database_->SaveMessage(record)) throw std::runtime_error("无法保存图片消息记录");
    const size_t count = (payload.size() + 511) / 512;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        job->acked.assign(count, false); job->sent.assign(count, false);
    }
    Publish(task, "progress", 0);
    LogMessage("IMAGE", "INFO", "Send inline image id=" + task.imageId + " fragments=" + std::to_string(count));
    const std::string reference = "/~#>" + task.imageId + "<B~{/font;-8 0 0 0 400 0 0 0 134 0 0 2 32 微软雅黑 8404992;}";
    int referenceTries = 0;
    auto sendReference = [&] {
        std::lock_guard<std::mutex> lock(mutex_);
        if (stopping_) throw std::runtime_error("图片发送已取消");
        if (!job->referenceAck && referenceTries < 4) {
            ++referenceTries;
            const uint64_t packet = messages_->SendImagePacket(task.target, IPMSG_SENDMSG | IPMSG_SENDCHECKOPT,
                                                               reference, job->referencePacket);
            if (!packet) throw std::runtime_error("图片引用发送失败");
            job->referencePacket = packet;
        }
    };
    // Transfer the image before inserting its chat reference. In the successful
    // native FeiQ capture, the full image and fragment ACKs precede the reference.
    // This lets the peer load the preview immediately instead of an object icon.
    LogMessage("IMAGE", "INFO", "Sending image data before reference id=" + task.imageId);
    auto lastProgress = std::chrono::steady_clock::now();
    for (size_t base = 0; base < count; base += 32) {
        const size_t end = (std::min)(base + 32, count);
        bool complete = false;
        for (int attempt = 0; attempt < 4 && !complete; ++attempt) {
            for (size_t i = base; i < end; ++i) {
                {
                    std::lock_guard<std::mutex> lock(mutex_);
                    if (stopping_) throw std::runtime_error("图片发送已取消");
                    if (job->acked[i]) continue;
                    job->sent[i] = true;
                }
                const size_t offset = i * 512, length = (std::min)(size_t(512), payload.size() - offset);
                std::string body = task.imageId + "|" + std::to_string(payload.size()) + "|" +
                    std::to_string(offset) + "|" + std::to_string(count) + "|" + std::to_string(i + 1) +
                    "|" + std::to_string(length) + "|0|1|0|00000000#";
                body.push_back('\0'); body.append(payload, offset, length);
                if (!messages_->SendImagePacket(task.target, IPMSG_SENDIMAGE | IPMSG_FILEATTACHOPT, body))
                    throw std::runtime_error("图片分片发送失败");
                if (WaitStopped(2)) throw std::runtime_error("图片发送已取消");
            }
            int progress = 0;
            {
                std::unique_lock<std::mutex> lock(mutex_);
                auto all = [&] { return std::all_of(job->acked.begin() + base, job->acked.begin() + end,
                                                     [](bool v) { return v; }); };
                changed_.wait_for(lock, std::chrono::seconds(1), [&] { return stopping_ || all(); });
                if (stopping_) throw std::runtime_error("图片发送已取消");
                complete = all();
                progress = static_cast<int>(job->acknowledged * 100 / count);
            }
            const auto now = std::chrono::steady_clock::now();
            if (now - lastProgress >= std::chrono::milliseconds(100)) {
                Publish(task, "progress", (std::min)(99, progress)); lastProgress = now;
            }
        }
        if (!complete) throw std::runtime_error("图片接收确认超时，对方可能不支持内嵌图片；可改用文件发送");
    }
    LogMessage("IMAGE", "INFO", "Image fragments acknowledged; sending reference id=" + task.imageId);
    bool referenceReady = false;
    for (int attempt = 0; attempt < 4; ++attempt) {
        sendReference();
        std::unique_lock<std::mutex> lock(mutex_);
        changed_.wait_for(lock, std::chrono::seconds(1), [&] { return stopping_ || job->referenceAck; });
        if (stopping_) throw std::runtime_error("图片发送已取消");
        if (job->referenceAck) {
            referenceReady = true;
            break;
        }
    }
    if (!referenceReady) throw std::runtime_error("图片已传输，但图片引用确认超时");
    // The reference and every fragment have now been acknowledged. This is
    // transport completion, not proof that the peer has rendered the preview.
    if (!database_->UpdateStatus(task.messageId, kMsgStatusCompleted)) throw std::runtime_error("无法更新图片送达状态");
    Publish(task, "completed", 100);
    LogMessage("IMAGE", "INFO", "Image acknowledged id=" + task.imageId);
}

} // namespace ipmsg
