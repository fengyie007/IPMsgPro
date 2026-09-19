#pragma once
// ============================================================================
// Screen capture (GDI + GDI+)
// ============================================================================

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <Windows.h>

#include <vector>

namespace ipmsg {

/// Capture the whole of one monitor and encode it as PNG.
/// Returns false when the monitor is invalid or GDI+ encoding fails.
bool CaptureMonitorToPng(HMONITOR hMonitor, std::vector<unsigned char>& outPng);

}  // namespace ipmsg
