// ============================================================================
// Network Utility Functions Implementation
// ============================================================================

// WinSock2 must come before Windows.h (included via network.h)
#include "network.h"
#include "logger.h"
#include "util/encoding.h"
#include <Windows.h>
#include <Lmcons.h>  // UNLEN
#include <regex>
#include <algorithm>

namespace ipmsg {

bool WSAInit() {
    WSADATA wsaData;
    int result = WSAStartup(MAKEWORD(2, 2), &wsaData);
    if (result != 0) {
        return false;
    }
    // Confirm WinSock 2.2
    if (LOBYTE(wsaData.wVersion) != 2 || HIBYTE(wsaData.wVersion) != 2) {
        WSACleanup();
        return false;
    }
    return true;
}

void WSACleanup() {
    ::WSACleanup();
}

std::vector<LocalAddress> GetLocalAddresses() {
    std::vector<LocalAddress> addresses;

    const ULONG flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    ULONG bufLen = 0;
    GetAdaptersAddresses(AF_INET, flags, nullptr, nullptr, &bufLen);
    if (bufLen == 0) {
        LogMessage("NETWORK", "WARN", "[Network] GetAdaptersAddresses returned zero buffer length");
        return addresses;
    }

    std::vector<uint8_t> buffer(bufLen);
    auto adapters = reinterpret_cast<PIP_ADAPTER_ADDRESSES>(buffer.data());

    ULONG ret = GetAdaptersAddresses(AF_INET, flags, nullptr, adapters, &bufLen);
    if (ret != ERROR_SUCCESS) {
        LogMessage("NETWORK", "WARN", "[Network] GetAdaptersAddresses failed, error=" + std::to_string(ret));
        return addresses;
    }

    LogMessage("NETWORK", "DEBUG", "[Network] Scanning network adapters...");
    for (auto adapter = adapters; adapter; adapter = adapter->Next) {
        std::string adapterName = adapter->AdapterName ? adapter->AdapterName : "(unknown)";
        LogMessage("NETWORK", "DEBUG", std::string("[Network] Adapter: ") + adapterName +
                   ", Status=" + std::string(adapter->OperStatus == IfOperStatusUp ? "UP" : "DOWN") +
                   ", Type=" + std::to_string(adapter->IfType));

        if (adapter->OperStatus != IfOperStatusUp) continue;
        if (adapter->IfType == IF_TYPE_SOFTWARE_LOOPBACK) {
            LogMessage("NETWORK", "DEBUG", "[Network] Skipping loopback adapter");
            continue;
        }

        for (auto addr = adapter->FirstUnicastAddress; addr; addr = addr->Next) {
            if (addr->Address.lpSockaddr->sa_family != AF_INET) continue;

            auto sa = reinterpret_cast<sockaddr_in*>(addr->Address.lpSockaddr);
            char ipStr[INET_ADDRSTRLEN] = {};
            inet_ntop(AF_INET, &sa->sin_addr, ipStr, sizeof(ipStr));
            LocalAddress la;
            la.ip = ipStr;
            la.prefixLength = static_cast<int>(addr->OnLinkPrefixLength);
            addresses.push_back(la);
            LogMessage("NETWORK", "DEBUG", "[Network] Found local IP: " + la.ip + "/" + std::to_string(la.prefixLength));
        }
    }

    LogMessage("NETWORK", "DEBUG", "[Network] Total local IPs found: " + std::to_string(addresses.size()));
    return addresses;
}

std::vector<std::string> GetLocalIPAddresses() {
    std::vector<std::string> ips;
    for (const auto& la : GetLocalAddresses()) {
        ips.push_back(la.ip);
    }
    return ips;
}

std::string GetBroadcastAddress(const std::string& ip, int prefixLength) {
    uint32_t ipHost = IPToUint32(ip);  // host byte order
    if (ipHost == 0) return "";

    // Unknown prefix: assume the common /24. Point-to-point links (/31, /32)
    // have no directed broadcast address.
    if (prefixLength <= 0) prefixLength = 24;
    if (prefixLength >= 31) return "";

    uint32_t mask = 0xFFFFFFFFu << (32 - prefixLength);
    uint32_t broadcast = (ipHost & mask) | ~mask;
    return Uint32ToIP(broadcast);
}

std::vector<std::string> GetAllBroadcastAddresses() {
    std::vector<std::string> broadcasts;

    // Always include limited broadcast (works within same subnet)
    broadcasts.push_back("255.255.255.255");

    // Directed broadcast per local interface, using the real subnet mask so
    // /16 or /22 networks are covered instead of an assumed /24.
    for (const auto& la : GetLocalAddresses()) {
        std::string bc = GetBroadcastAddress(la.ip, la.prefixLength);
        if (!bc.empty() && bc != "255.255.255.255") {
            // Avoid duplicates
            if (std::find(broadcasts.begin(), broadcasts.end(), bc) == broadcasts.end()) {
                broadcasts.push_back(bc);
                LogMessage("NETWORK", "DEBUG", "[Network] Directed broadcast for " + la.ip + "/" +
                           std::to_string(la.prefixLength) + " -> " + bc);
            }
        }
    }

    LogMessage("NETWORK", "DEBUG", "[Network] Broadcast addresses: " + std::to_string(broadcasts.size()));
    return broadcasts;
}

uint32_t IPToUint32(const std::string& ip) {
    uint32_t result = 0;
    int ret = inet_pton(AF_INET, ip.c_str(), &result);
    if (ret != 1) return 0;
    return ntohl(result);  // Convert network byte order to host byte order
}

std::string Uint32ToIP(uint32_t ip) {
    uint32_t netIp = htonl(ip);  // Convert host byte order to network byte order
    char buf[INET_ADDRSTRLEN] = {};
    inet_ntop(AF_INET, &netIp, buf, sizeof(buf));
    return buf;
}

std::string GetHostName() {
    wchar_t buf[MAX_COMPUTERNAME_LENGTH + 1] = {};
    DWORD size = MAX_COMPUTERNAME_LENGTH + 1;
    if (!GetComputerNameW(buf, &size)) return "";
    // Return UTF-8. The rest of the app treats localUser.hostName as UTF-8,
    // and MakeMsg converts it to GBK for FeiQ. Using the *A variant here would
    // return GBK bytes that get double-encoded into mojibake.
    return enc::WideToUtf8(std::wstring(buf, size));
}

std::string GetUserName() {
    wchar_t buf[UNLEN + 1] = {};
    DWORD size = UNLEN + 1;
    if (!::GetUserNameW(buf, &size)) return "";
    // GetUserNameW's size includes the terminating NUL.
    return enc::WideToUtf8(std::wstring(buf, size > 0 ? size - 1 : 0));
}

std::string GetLocalMacAddress() {
    std::string mac;

    ULONG bufLen = 0;
    GetAdaptersAddresses(AF_INET, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST |
                         GAA_FLAG_SKIP_DNS_SERVER, nullptr, nullptr, &bufLen);
    if (bufLen == 0) return mac;

    std::vector<uint8_t> buffer(bufLen);
    auto adapters = reinterpret_cast<PIP_ADAPTER_ADDRESSES>(buffer.data());
    ULONG ret = GetAdaptersAddresses(AF_INET, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST |
                                     GAA_FLAG_SKIP_DNS_SERVER, nullptr, adapters, &bufLen);
    if (ret != ERROR_SUCCESS) return mac;

    for (auto adapter = adapters; adapter; adapter = adapter->Next) {
        if (adapter->OperStatus != IfOperStatusUp) continue;
        if (adapter->IfType == IF_TYPE_SOFTWARE_LOOPBACK) continue;
        if (adapter->PhysicalAddressLength == 0) continue;

        // Format as uppercase hex without separators, e.g. "30B49EAE34C4"
        char hex[18] = {};
        static const char* digits = "0123456789ABCDEF";
        for (ULONG i = 0; i < adapter->PhysicalAddressLength && i < 6; ++i) {
            hex[i * 2]     = digits[(adapter->PhysicalAddress[i] >> 4) & 0xF];
            hex[i * 2 + 1] = digits[adapter->PhysicalAddress[i] & 0xF];
        }
        mac = hex;
        break;
    }

    return mac;
}

} // namespace ipmsg
