#pragma once
// ============================================================================
// Notification sound (embedded notification.mp3, played through MCI)
// ============================================================================

namespace ipmsg {

/// Play the new-message sound. Asynchronous and fire-and-forget: the MCI
/// device is released a few seconds later on a detached thread. Safe to call
/// from any thread.
void PlayNotificationSound();

}  // namespace ipmsg
