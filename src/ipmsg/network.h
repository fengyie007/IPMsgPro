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

/// A local IPv4 address together with its on-link prefix length.
struct LocalAddress {
    std::string ip;
    int prefixLength = 0;   // e.g. 24 for 255.255.255.0; 0 when unknown
};

/// Get all local IPv4 addresses (adapters that are up, excluding loopback)
/// with their prefix lengths, as reported by GetAdaptersAddresses.
std::vector<LocalAddress> GetLocalAddresses();

/// Get all local IPv4 addresses (convenience wrapper over GetLocalAddresses)
std::vector<std::string> GetLocalIPAddresses();

/// Get the directed broadcast address for an IP and prefix length
/// (e.g. "10.8.34.11"/24 -> "10.8.34.255", "10.0.0.5"/16 -> "10.0.255.255").
/// prefixLength <= 0 falls back to /24; >= 31 (point-to-point) returns "".
std::string GetBroadcastAddress(const std::string& ip, int prefixLength = 24);

/// Get all broadcast addresses for all local interfaces
/// (limited broadcast 255.255.255.255 plus one directed broadcast per interface)
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
