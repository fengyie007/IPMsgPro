# AGENTS.md

## Build

Requires: VS2022 (C++ desktop workload), CMake 3.15+, Python 3, Node.js 18+.

```powershell
.\build.ps1 -Arch x64          # x64 Release → build_x64/Release/IPMsgPro.exe
.\build.ps1 -Arch x86          # x86 Release → build_x86/Release/IPMsgPro_X86.exe
.\build.ps1 -Arch x64 -SkipFrontend   # skip frontend if unchanged
.\build.ps1 -Arch x64 -Run      # build + launch
```

Frontend-only:
```powershell
cd frontend
npm install
npx vite build    # or: npm run build (runs tsc -b first)
```

## Architecture

- **Backend**: C++17 + Win32 + WebView2 via TauriCPP framework (`TauriCPP/` submodule)
- **Frontend**: React + TypeScript + Tailwind CSS + Vite (`frontend/`)
- Frontend `dist/` is packed into the exe as embedded resources by `TauriCPP/tools/pack_resources.py`
- CMake builds per-architecture static libs (`tauricpp_x64`, `sqlite3_x64`) linked into `IPMsgPro`
- Entry point: `src/main.cpp` (WinMain)

## Key directories

- `src/` — C++ backend (protocol, bridge, database, file transfer)
- `src/ipmsg/` — IPMsg v3.0 protocol (UDP 2425)
- `src/bridge/` — frontend ↔ backend command bridge
- `src/database/` — SQLite message storage
- `src/file/` — TCP file transfer
- `frontend/src/components/` — React UI components
- `frontend/src/stores/` — Zustand state management
- `frontend/src/services/` — backend bridge services (IPC)
- `resources/` — app icons, embedded notification sounds

## Frontend ↔ Backend IPC

The frontend communicates with C++ via `tauricpp::Bridge`. Commands are registered in `src/bridge/command_handler.cpp`. Frontend services in `frontend/src/services/` invoke these commands.

## Data paths

- Logs: `%LOCALAPPDATA%\.ipmsgpro\ipmsg_gui_debug.log`
- Database: `%LOCALAPPDATA%\.ipmsgpro\ipmsg.db`
- Custom port: `.ipmsgpro_<port>` suffix

## Notes

- No test suite exists. No lint/typecheck CI.
- `WinSock2.h` must be included before `Windows.h` (see `src/main.cpp:6-9`)
- Static CRT linking (`/MT`) — no runtime DLL dependencies
- Frontend build outputs to `frontend/dist/` (gitignored)
- Emoji data: `frontend/src/emojiData.ts` generated from `frontend/scripts/gen_emoji_ts.cjs`
