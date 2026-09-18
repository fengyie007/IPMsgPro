// ============================================================================
// Network Utility Functions Implementation
// ============================================================================

// WinSock2 must come before Windows.h (included via network.h)
#include "network.h"
#include "logger.h"
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

std::vector<std::string> GetLocalIPAddresses() {
    std::vector<std::string> addresses;

    ULONG bufLen = 0;
    GetAdaptersAddresses(AF_INET, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST |
                         GAA_FLAG_SKIP_DNS_SERVER, nullptr, nullptr, &bufLen);
    if (bufLen == 0) {
        LogMessage("NETWORK", "", "[Network] GetAdaptersAddresses returned zero buffer length");
        return addresses;
    }

    std::vector<uint8_t> buffer(bufLen);
    auto adapters = reinterpret_cast<PIP_ADAPTER_ADDRESSES>(buffer.data());

    ULONG ret = GetAdaptersAddresses(AF_INET, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST |
                                     GAA_FLAG_SKIP_DNS_SERVER, nullptr, adapters, &bufLen);
    if (ret != ERROR_SUCCESS) {
        LogMessage("NETWORK", "", "[Network] GetAdaptersAddresses failed, error=" + std::to_string(ret));
        return addresses;
    }

    LogMessage("NETWORK", "", "[Network] Scanning network adapters...");
    for (auto adapter = adapters; adapter; adapter = adapter->Next) {
        std::string adapterName = adapter->AdapterName ? adapter->AdapterName : "(unknown)";
        LogMessage("NETWORK", "", std::string("[Network] Adapter: ") + adapterName +
                   ", Status=" + std::string(adapter->OperStatus == IfOperStatusUp ? "UP" : "DOWN") +
                   ", Type=" + std::to_string(adapter->IfType));

        if (adapter->OperStatus != IfOperStatusUp) continue;
        if (adapter->IfType == IF_TYPE_SOFTWARE_LOOPBACK) {
            LogMessage("NETWORK", "", "[Network] Skipping loopback adapter");
            continue;
        }

        for (auto addr = adapter->FirstUnicastAddress; addr; addr = addr->Next) {
            if (addr->Address.lpSockaddr->sa_family != AF_INET) continue;

            auto sa = reinterpret_cast<sockaddr_in*>(addr->Address.lpSockaddr);
            char ipStr[INET_ADDRSTRLEN] = {};
            inet_ntop(AF_INET, &sa->sin_addr, ipStr, sizeof(ipStr));
            addresses.push_back(ipStr);
            LogMessage("NETWORK", "", "[Network] Found local IP: " + std::string(ipStr));
        }
    }

    LogMessage("NETWORK", "", "[Network] Total local IPs found: " + std::to_string(addresses.size()));
    return addresses;
}

std::string GetBroadcastAddress(const std::string& ip) {
    uint32_t ipVal = IPToUint32(ip);
    if (ipVal == 0) return "";

    // Convert to host byte order for calculations
    uint32_t ipHost = ntohl(ipVal);
    
    // Simple heuristic: assume /24 subnet for common home/office networks
    // 255.255.255.0 in host byte order
    uint32_t mask = 0xFFFFFF00;
    uint32_t network = ipHost & mask;
    uint32_t broadcast = network | 0x000000FF;

    // Convert back to network byte order for output
    return Uint32ToIP(htonl(broadcast));
}

std::vector<std::string> GetAllBroadcastAddresses() {
    std::vector<std::string> broadcasts;

    // Always include limited broadcast (works within same subnet)
    broadcasts.push_back("255.255.255.255");

    // Calculate directed broadcast for each local interface
    auto localIPs = GetLocalIPAddresses();
    for (const auto& ip : localIPs) {
        std::string bc = GetBroadcastAddress(ip);
        if (!bc.empty() && bc != "255.255.255.255") {
            // Avoid duplicates
            if (std::find(broadcasts.begin(), broadcasts.end(), bc) == broadcasts.end()) {
                broadcasts.push_back(bc);
                LogMessage("NETWORK", "", "[Network] Directed broadcast for " + ip + " -> " + bc);
            }
        }
    }

    LogMessage("NETWORK", "", "[Network] Broadcast addresses: " + std::to_string(broadcasts.size()));
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

bool IsInSubnet(const std::string& ip, const std::string& subnet) {
    // Parse CIDR: "192.168.1.0/24"
    size_t slashPos = subnet.find('/');
    if (slashPos == std::string::npos) return ip == subnet;

    std::string netPart = subnet.substr(0, slashPos);
    int prefixLen = std::stoi(subnet.substr(slashPos + 1));

    uint32_t ipVal = IPToUint32(ip);
    uint32_t netVal = IPToUint32(netPart);

    if (prefixLen == 0) return true;
    uint32_t mask = (prefixLen >= 32) ? 0xFFFFFFFF : (0xFFFFFFFF << (32 - prefixLen));

    return (ipVal & mask) == (netVal & mask);
}

std::string GetHostName() {
    wchar_t buf[MAX_COMPUTERNAME_LENGTH + 1] = {};
    DWORD size = MAX_COMPUTERNAME_LENGTH + 1;
    if (!GetComputerNameW(buf, &size)) return "";
    // Return UTF-8. The rest of the app treats localUser.hostName as UTF-8,
    // and MakeMsg converts it to GBK for FeiQ via UTF8ToGBK. Using the *A
    // variant here would return GBK bytes that get double-encoded into mojibake.
    int len = WideCharToMultiByte(CP_UTF8, 0, buf, -1, nullptr, 0, nullptr, nullptr);
    if (len <= 0) return "";
    std::string out(len - 1, '\0');
    WideCharToMultiByte(CP_UTF8, 0, buf, -1, &out[0], len, nullptr, nullptr);
    return out;
}

std::string GetUserName() {
    wchar_t buf[UNLEN + 1] = {};
    DWORD size = UNLEN + 1;
    if (!::GetUserNameW(buf, &size)) return "";
    int len = WideCharToMultiByte(CP_UTF8, 0, buf, -1, nullptr, 0, nullptr, nullptr);
    if (len <= 0) return "";
    std::string out(len - 1, '\0');
    WideCharToMultiByte(CP_UTF8, 0, buf, -1, &out[0], len, nullptr, nullptr);
    return out;
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
