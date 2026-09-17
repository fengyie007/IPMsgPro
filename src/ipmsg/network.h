#pragma once
// ============================================================================
// Network Utility Functions
// Provides cross-platform network helpers for IPMsg
// ============================================================================

#include <string>
#include <vector>
#include <cstdint>

// WinSock2 must be included before Windows.h
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <WinSock2.h>
#include <WS2tcpip.h>
#include <iphlpapi.h>
#pragma comment(lib, "ws2_32.lib")
#pragma comment(lib, "iphlpapi.lib")

namespace ipmsg {

/// Initialize Winsock (call once at startup)
bool WSAInit();

/// Cleanup Winsock (call once at shutdown)
void WSACleanup();

/// Get all local IPv4 addresses
std::vector<std::string> GetLocalIPAddresses();

/// Get subnet broadcast address for a given IP (e.g., "192.168.1.100" -> "192.168.1.255")
std::string GetBroadcastAddress(const std::string& ip);

/// Get all broadcast addresses for all local interfaces
std::vector<std::string> GetAllBroadcastAddresses();

/// Convert IP string to uint32_t (network byte order)
uint32_t IPToUint32(const std::string& ip);

/// Convert uint32_t to IP string (network byte order)
std::string Uint32ToIP(uint32_t ip);

/// Check if an IP address is in a given subnet (CIDR notation, e.g., "192.168.1.0/24")
bool IsInSubnet(const std::string& ip, const std::string& subnet);

/// Get hostname of the local machine
std::string GetHostName();

/// Get username of the current logged-in user
std::string GetUserName();

/// Get the MAC address of the first active (non-loopback) adapter,
/// formatted as uppercase hex without separators (e.g. "30B49EAE34C4")
std::string GetLocalMacAddress();

} // namespace ipmsg
