# AGENTS.md

`CLAUDE.md` is a symlink to this file — edit `AGENTS.md`.

## Build

Requires: Visual Studio 2022+ with the C++ desktop workload, CMake 3.15+, Python 3, Node.js 18+. `build.ps1` detects the VS version via vswhere (VS2026 Build Tools use their bundled cmake; otherwise vcvarsall + Ninja fallback). The WebView2 SDK is vendored in `third_party/WebView2/{x64,x86}` — no vcpkg needed. The x86 target additionally needs the MSVC x86 CRT libraries installed (`VC/Tools/MSVC/<ver>/lib/x86`); without them the x86 configure step fails with `LNK1104: MSVCRTD.lib`.

```powershell
.\build.ps1 -Arch x64                  # x64 Release → build_x64/Release/SpeedIpMsg.exe
.\build.ps1 -Arch x86                  # x86 Release → build_x86/Release/SpeedIpMsg_X86.exe
.\build.ps1 -Arch x64 -SkipFrontend    # skip frontend build if unchanged
.\build.ps1 -Arch x64 -Run -Port 2426  # build + launch (optional custom port)
# other flags: -Config Debug|Release, -Clean
```

`build.ps1` runs `npx vite build` only — TypeScript errors are NOT reported. Type-check with `cd frontend && npm run build` (`tsc -b && vite build`). `npm run dev` serves the UI at :5173 with the bridge mocked. Incremental backend-only rebuilds: `cmake --build build_x64 --config Release --target SpeedIpMsg` (use the Build Tools cmake if that generated the tree).

## Architecture

- **Backend**: C++17 + Win32 + WebView2 via TauriCPP. `TauriCPP/` is a vendored copy (not a submodule) of github.com/masonwu21/TauriCPP with local changes (tray icon, native `IDropTarget` drag-drop, window icon, UTF-8 `homeDir`/`defaultDataDir` injection in `bridge.cpp`) — treat it as project code.
- **Frontend**: React 18 + TypeScript + Tailwind + Vite + Zustand (`frontend/`)
- `frontend/dist/` is packed into the exe as Windows resources by `TauriCPP/tools/pack_resources.py`; `build.ps1` repacks before every build
- CMake builds per-architecture static libs (`tauricpp_<arch>`, `sqlite3_<arch>`) linked into `SpeedIpMsg`
- Entry point: `src/main.cpp` (WinMain). Static CRT (`/MT`), no runtime DLLs. Devtools are enabled even in Release (F12).

## Key directories

- `src/ipmsg/` — IPMsg v3.0 protocol over UDP 2425 (`protocol.h` constants, `msgmng.cpp` discovery/messaging/IP-range scan, `network.cpp` sockets + directed-broadcast helpers using the real subnet prefix)
- `src/bridge/command_handler.cpp` — command registration, event forwarding, config handling, all `Handle*` commands
- `src/bridge/feiq_screenshot.*` — FeiQ inline-image reassembly and fragment ACKs; `src/bridge/feiq_image_sender.*` — bounded background send queue, GDI+ image conversion, ACK/retry handling
- `src/util/feiq_lzw.*` — shared FeiQ LZW encode/decode + CRC32 (cyclic dictionary slots 256–4095, 12-bit width after saturation)
- `src/bridge/screen_capture.*`, `src/bridge/notification_sound.*` — GDI+ screen capture, MCI notification sound
- `src/util/encoding.*` — the only place that calls `MultiByteToWideChar`/`WideCharToMultiByte`; `src/util/app_paths.*` — data dir, registry, Downloads, temp; `src/util/base64.*`
- `src/database/` — SQLite message storage (single `messages` table, `MessageStatus` enum, thread-safe)
- `src/file/` — TCP file transfer (accept thread + detached send/recv workers, counted so `Shutdown()` can wait)
- `src/cli_runner.*` — headless `--mode=cli` protocol harness; `src/logger.*` — unified logger with levels
- `frontend/src/components/` — `ChatPanel` (conversation view), `MessageBubble` (text/image/file bubbles), `Settings`, `UserListPanel`, `LeftSidebar`, `Toast`, `ConfirmDialog`, `ScreenshotEditor`
- `frontend/src/stores/` — Zustand: `messageStore` (messages, unread, history paging, pending receives), `userStore`, `configStore`, `toastStore`
- `frontend/src/services/` — `bridge.ts` (IPC + dev mocks), `configDB.ts` (IndexedDB); `frontend/src/utils/` — `format`, `netValidation`
- `resources/` — icon, `app.rc` (version info, embedded `notification.mp3`)

## Frontend ↔ Backend IPC

- Commands are registered in `CommandHandler::RegisterAllCommands()`; names are `domain.action` (`user.*`, `message.*`, `file.*`, `history.*`, `network.*`, `config.*`, `dialog.*`, `window.*`, `screenshot.*`, `shell_open`). Handlers take and return `nlohmann::json` (`{success, error?, ...}`); the frontend receives the result already parsed.
- Backend → frontend events go through `bridge_->Emit("x.y", json)` (`message.received`, `message.ack`, `file.*`, `user.*`, `network.scan_*`, `feiq.screenshot_received`); the frontend subscribes with `listen()` from `services/bridge.ts` (mostly inside each store's `initListeners`).
- `services/bridge.ts` falls back to `getMockResponse()` when `window.__tauricpp__` is absent — add a mock case for every new command, or dev mode returns "Unknown command".
- Handlers run on the WebView2 UI thread; `MsgMng` callbacks run on the UDP receive thread; file-transfer callbacks on TCP worker threads. `Bridge::Emit` is thread-safe (posts to the UI thread).

## Inline image sending

- `image.send {target, filePath}` queues PNG/JPEG/BMP as a FeiQ inline image; screenshots save PNG first and use the same command. Normal files stay on TCP.
- For direct FeiQ preview, send/ACK all binary fragments first, then send the single text reference (0x120, two trailing NULs). Reference-first produced a generic object icon in verified tests; data-first fixed it without encryption.
- `image.send_progress/completed/failed` use `messageId` (not a TCP transferId). `success` from `image.send` only means queued; completion requires fragment ACKs and the reference receipt, not proof of rendering/read status.
- Sent image copies live in `<dataDir>/images/<messageId>/`. Call `CommandHandler::Shutdown` before destroying network/database objects.
- Limits: 4 queued/active sends, 20 MiB input, 16M pixels, 16 MiB compressed payload. The sender paces 512-byte fragments with bounded retries; unsupported peers fail explicitly rather than silently falling back to files.

## Config flow

Settings live in the frontend IndexedDB (`ipmsg-config`), not in C++. The backend only learns values via one `config.set` call at startup (replayed from IndexedDB) and one per save. Adding a config field:
1. `Config` + `DEFAULT_CONFIG` in `frontend/src/types/index.ts` — `configDB.loadConfig` and `configStore` pick it up automatically
2. If the backend needs it: add the key to `BACKEND_KEYS` in `configStore.ts` and handle it in `HandleConfigSet` (only keys present in the request are applied)
3. UI in `frontend/src/components/Settings.tsx` (list inputs go through `utils/netValidation.ts`)

After the startup sync the frontend calls `config.loaded`; the backend then sends BR_ENTRY to `directUsers` and starts the `ipScanRanges` scan itself (the frontend must not start a second one). The listening port is a process setting (`--port`), not a config field. IndexedDB is stored under `%TEMP%\tauricpp_<exe name>\`, so renaming the exe resets all user settings.

## Data paths

- Data dir: `%USERPROFILE%\.speedipmsg` (overridden by registry `HKCU\Software\SpeedIPMsg\DataDir`; a non-default port appends `_<port>`). Resolved by `paths::ResolveDataDir`, always UTF-8.
- Log: `<dataDir>\ipmsg_gui_debug.log` — truncated on every start; `std::cout`/`cerr` are redirected into it
- Database: `<dataDir>\ipmsg.db`
- Received files: `%USERPROFILE%\Downloads` (peer-supplied names are sanitized and never overwrite; FeiQ inline screenshots go to `Downloads\IPMsgPro`). Protocol diagnostics (`FeiQ_*.bin`) are written to `<dataDir>\debug\` only with `--verbose`.

## Conventions

- **Logging**: only `ipmsg::LogMessage(tag, level, msg)` from `src/logger.h`; level is `"DEBUG"`, `""`/`"INFO"`, `"WARN"`, `"ERROR"`. Default threshold is INFO; `--verbose` enables DEBUG. Anything per-packet, per-chunk or per-progress-tick must be DEBUG, and expensive formatting must be wrapped in `if (IsDebugEnabled())` — the receive thread once dropped FeiQ fragments because of synchronous per-packet logging.
- **Encoding**: all in-memory strings are UTF-8. On the wire, encode GBK unless `IPMSG_UTF8OPT` is set; file-attach messages never set it (FeiQ can't read it). Use `enc::*` from `util/encoding.h` (`Utf8ToWide`, `PathFromUtf8`, `Utf8ToAnsi`, ...) and the `*W` Win32 APIs; never call `*A` APIs with UTF-8 or add another local conversion helper.
- **Threads**: file send/recv run on detached `std::thread`s tracked by `activeWorkers_`. Wrap thread bodies and `bridge_->Emit` callbacks in try/catch — an uncaught exception is `std::terminate` with no UI. Never leave a `std::thread` joinable at destruction (the scan thread crash). `MessageDB` methods lock internally; read `MsgMng` local user info via `GetLocalUser()` (a snapshot).
- **Message ids**: sent text = `<ms>_<rand>`; received = `<senderKey>:<packetNo>`; files = transferId (both directions). `MessageStatus` (0 sending / 1 delivered / 2 completed / 3 failed) is written back on RECVMSG receipt and transfer completion/failure.
- `WinSock2.h` must be included before `Windows.h` (see `src/main.cpp:5-11`).
- **Version bump** (keep all in sync): `src/main.cpp` `kAppVersion`, `frontend/package.json`, `frontend/src/types/index.ts` `APP_VERSION`, `resources/app.rc` (numeric `FILEVERSION`/`PRODUCTVERSION` and the string values), `CMakeLists.txt` `project(... VERSION)`, `README.md` title + 更新日志.
- **Frontend**: user-facing errors via `toast.*` from `stores/toastStore.ts`, confirmations via `ConfirmDialog` (no `alert`/`confirm`); `console.log` tracing is kept on purpose (devtools are available in Release via F12); colors/widths come from the `tailwind.config.js` tokens (`sidebar.*`, `list.bg`, `chat.*`, `w-sidebar`, `w-user-list`).

## Testing & debugging

- No CI. `tools/test_feiq_lzw.cpp` is a standalone portable codec regression test (build instructions at its top; optional real LZW/BMP fixtures stay outside git). Verify the full app by building/running and checking `ipmsg_gui_debug.log` (`--verbose` for protocol traces).
- Two-instance test: `SpeedIpMsg.exe --port=2426` next to the default instance (separate data dir + DB).
- Headless protocol harness: `SpeedIpMsg.exe --mode=cli --cmd=server|test --port=N [--config=<json>] [--target=ip[:port]]`; `tools/e2e_smoke.py <exe> <gui_port> <cli_port>` drives a CLI runner against a GUI instance and prints the resulting log lines and DB rows.
- GUI flags: `--port=N` (also isolates the data dir), `--adduser=ip:port[,ip:port...]` (skips broadcast, sends BR_ENTRY directly), `--verbose`.

## Working docs

- `todo.md` — per-task change log (problem → root cause → files changed); `README.md` 更新日志 — user-facing changelog; `plan.md` — original product spec.
- Emoji data: `frontend/src/emojiData.ts` is generated from `assets/emoji_positon.less` via `node scripts/gen_emoji_ts.cjs`; don't hand-edit.
- Repo is jj-colocated (`.jj/` is git-ignored globally); use git normally.
