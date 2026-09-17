#pragma once
#include <string>
#include <functional>
#include <memory>
#include <atomic>
#include <queue>
#include <mutex>
#include <Windows.h>
#include <shellapi.h>  // NOTIFYICONDATAW, Shell_NotifyIconW, ShellExecuteW

// WebView2 前向声明
struct ICoreWebView2;
struct ICoreWebView2Controller;
struct ICoreWebView2Environment;
struct ICoreWebView2WebResourceRequest;
struct ICoreWebView2WebResourceResponse;

namespace tauricpp {

class Bridge;

/// 窗口类 - 封装Win32窗口 + WebView2
/// 核心功能：
/// 1. 创建Win32原生窗口
/// 2. 初始化WebView2环境
/// 3. 拦截WebResourceRequested，从VirtualFS提供内容
/// 4. 桥接Bridge实现双向通信
class Window {
public:
    using CloseCallback = std::function<bool()>;  ///< 返回true允许关闭，false阻止关闭
    using ResizeCallback = std::function<void(int w, int h)>;
    using MinimizeCallback = std::function<void()>;
    using MaximizeCallback = std::function<void()>;
    using FocusCallback = std::function<void()>;

    struct Config {
        std::string title = "TauriCPP App";
        int width = 1024;
        int height = 768;
        bool center = true;
        bool resizable = true;
        bool always_on_top = false;
        bool devtools = false;          ///< 是否启用DevTools（F12切换）
        std::string start_url = "https://tauricpp.app/index.html";
        COLORREF bg_color = RGB(15, 12, 41);  ///< 默认背景色（匹配前端渐变起点），消除白屏
    };

    explicit Window(const Config& config = {});
    ~Window();

    // 禁止拷贝和移动
    Window(const Window&) = delete;
    Window& operator=(const Window&) = delete;
    Window(Window&&) = delete;
    Window& operator=(Window&&) = delete;

    /// 显示窗口并进入消息循环（阻塞）
    int Run();

    /// 关闭窗口
    void Close();

    /// 执行JavaScript代码
    void ExecuteJs(const std::string& js);

    /// 从队列取出并执行JS（UI线程调用）
    void ExecuteJsFromQueue();

    /// 获取窗口句柄
    HWND GetHwnd() const { return hwnd_; }

    // ---- 窗口操作API ----
    void SetTitle(const std::string& title);
    void SetSize(int width, int height);
    void SetPosition(int x, int y);
    void SetAlwaysOnTop(bool on_top);
    void SetResizable(bool resizable);
    void SetIcon(HICON icon);
    void Minimize();
    void Maximize();
    void Restore();
    void Hide();        ///< Hide window (for minimize-to-tray)
    void Show();        ///< Show window (restore from tray)
    bool IsMinimized() const;
    bool IsMaximized() const;
    bool IsVisible() const;
    bool IsFocused() const;

    // ---- 窗口生命周期回调 ----
    using CreatedCallback = std::function<void(Window&)>;  ///< Window created callback
    void OnCreated(CreatedCallback cb) { on_created_ = std::move(cb); }
    void OnClose(CloseCallback cb) { on_close_ = std::move(cb); }
    void OnResize(ResizeCallback cb) { on_resize_ = std::move(cb); }
    void OnMinimize(MinimizeCallback cb) { on_minimize_ = std::move(cb); }
    void OnMaximize(MaximizeCallback cb) { on_maximize_ = std::move(cb); }
    void OnFocus(FocusCallback cb) { on_focus_ = std::move(cb); }

    /// 切换DevTools（F12也会触发）
    void ToggleDevTools();

    // ---- 系统托盘 API ----
    /// Create system tray icon. iconPath is the .ico file path.
    void CreateTrayIcon(const std::string& iconPath, const std::string& tooltip = "");
    /// Remove system tray icon
    void RemoveTrayIcon();
    /// Update tray icon tooltip
    void SetTrayTooltip(const std::string& tooltip);
    /// Show balloon notification from tray
    void ShowTrayNotification(const std::string& title, const std::string& message);
    /// Set tray context menu items (label -> callback pairs). Show on right-click.
    void SetTrayMenu(const std::vector<std::pair<std::string, std::function<void()>>>& items);
    /// Tray click callback (left-click on tray icon)
    using TrayClickCallback = std::function<void()>;
    void OnTrayClick(TrayClickCallback cb) { on_tray_click_ = std::move(cb); }

private:
    /// 创建Win32窗口
    bool CreateNativeWindow();

    /// 初始化WebView2
    bool InitWebView();

    /// 设置WebResourceRequested拦截
    void SetupResourceInterception();

    /// 设置通信桥接
    void SetupBridge();

    /// 设置WebView2默认背景色（消除白屏闪烁）
    void SetWebViewBackgroundColor();

    /// 启用原生文件拖放：在主窗口注册 IDropTarget 接收拖入文件的真实路径，
    /// 并禁用 WebView2 自带的拖放处理（页面无法读取文件磁盘路径）。
    void EnableNativeFileDrop();

    /// 递归撤销指定窗口下所有子窗口（含深层浏览器子窗口）的拖放注册，
    /// 使拖放最终落到我们的 IDropTarget，避免被 WebView2 抢走。
    void RevokeChildDragDrop(HWND root);

    /// 把我们的 IDropTarget 注册到主窗口及所有子窗口（含 WebView2 深层窗口）。
    /// OLE 命中光标正下方的窗口，因此必须为每个可能被光标命中的窗口都注册，
    /// 否则撤销 WebView2 目标后该窗口无目标，drop 会被直接丢弃。
    void RegisterNativeDropTargets();

    /// Win32窗口过程
    static LRESULT CALLBACK WndProc(HWND hwnd, UINT msg, WPARAM wParam, LPARAM lParam);

    Config config_;
    HWND hwnd_ = nullptr;
    ICoreWebView2* webview_ = nullptr;
    ICoreWebView2Controller* controller_ = nullptr;
    ICoreWebView2Environment* env_ = nullptr;
    bool webview_ready_ = false;
    std::atomic<bool> shutting_down_{false};

    // 原生文件拖放：注册在主窗口上的 IDropTarget（禁用 WebView2 自带拖放，
    // 使窗口能收到拖入文件的真实路径）。以 void* 存储以避免在头文件中引入 COM 头。
    void* drop_target_ = nullptr;

    // 窗口生命周期回调
    CreatedCallback on_created_;
    CloseCallback on_close_;
    ResizeCallback on_resize_;
    MinimizeCallback on_minimize_;
    MaximizeCallback on_maximize_;
    FocusCallback on_focus_;

    // WebView2 事件令牌，用于注销
    struct EventTokens {
        EventTokens();
        ~EventTokens();
        struct Impl;
        std::unique_ptr<Impl> impl;
    } event_tokens_;

    // UI线程ID（用于投递JS执行）
    DWORD mainThreadId_ = 0;

    // 线程安全的JS执行队列
    std::mutex jsQueueMutex_;
    std::queue<std::string> jsQueue_;

    // 异步invoke请求队列（用于阻塞型命令如dialog）
    struct AsyncInvokeRequest {
        int id;
        std::string cmd;
        std::string args_json;
    };
    std::mutex asyncInvokeMutex_;
    std::queue<AsyncInvokeRequest> asyncInvokeQueue_;
    void ProcessAsyncInvokeQueue();

    // 系统托盘
    static constexpr UINT WM_TAURICPP_TRAY = WM_APP + 100;
    static constexpr UINT TRAY_FLASH_TIMER_ID = 1001;  ///< 定时器ID（SetTimer用wParam传递）
    NOTIFYICONDATAW trayIconData_ = {};
    bool trayIconCreated_ = false;
    std::vector<std::pair<std::string, std::function<void()>>> trayMenuItems_;
    TrayClickCallback on_tray_click_;
    HMENU trayMenu_ = nullptr;
    std::string trayOriginalTooltip_;
    void ProcessTrayMessage(WPARAM wParam, LPARAM lParam);

    // 托盘图标闪烁（像微信：图标在 有/无 之间交替）
    bool trayFlashing_ = false;
    bool flashShowIcon_ = true;          ///< true=显示原图标, false=显示空图标
    HICON trayOriginalIcon_ = nullptr;   ///< 保存原始图标
    HICON trayBlankIcon_ = nullptr;      ///< 全透明空图标
    void StartTrayFlash();
    void StopTrayFlash();
};

} // namespace tauricpp
