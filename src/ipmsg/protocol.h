#pragma once
// ============================================================================
// IPMsg Protocol Definitions
// Based on IPMsg v3.0 protocol, ported from ipmsg-master/src/ipmsg.h
// ============================================================================

#include <cstdint>

namespace ipmsg {

// ---------- Protocol Version ----------
constexpr uint32_t IPMSG_VERSION       = 0x0001;
constexpr uint32_t IPMSG_NEW_VERSION   = 0x0003;
constexpr uint32_t IPMSG_DEFAULT_PORT  = 0x0979;  // 2425

// ---------- Command Macros ----------
constexpr uint32_t GET_MODE(uint32_t cmd) { return cmd & 0x000000ffUL; }
constexpr uint32_t GET_OPT(uint32_t cmd)  { return cmd & 0xffffff00UL; }

// ---------- Command Functions (Low 8 bits) ----------
constexpr uint32_t IPMSG_NOOPERATION    = 0x00000000UL;

// User discovery
constexpr uint32_t IPMSG_BR_ENTRY       = 0x00000001UL;
constexpr uint32_t IPMSG_BR_EXIT        = 0x00000002UL;
constexpr uint32_t IPMSG_ANSENTRY       = 0x00000003UL;
constexpr uint32_t IPMSG_BR_ABSENCE     = 0x00000004UL;
constexpr uint32_t IPMSG_BR_NOTIFY      = IPMSG_BR_ABSENCE;

// Host list
constexpr uint32_t IPMSG_BR_ISGETLIST   = 0x00000010UL;
constexpr uint32_t IPMSG_OKGETLIST      = 0x00000011UL;
constexpr uint32_t IPMSG_GETLIST        = 0x00000012UL;
constexpr uint32_t IPMSG_ANSLIST        = 0x00000013UL;
constexpr uint32_t IPMSG_ANSLIST_DICT   = 0x00000014UL;
constexpr uint32_t IPMSG_BR_ISGETLIST2  = 0x00000018UL;

// Message
constexpr uint32_t IPMSG_SENDMSG        = 0x00000020UL;
constexpr uint32_t IPMSG_RECVMSG        = 0x00000021UL;
constexpr uint32_t IPMSG_READMSG        = 0x00000030UL;
constexpr uint32_t IPMSG_DELMSG         = 0x00000031UL;
constexpr uint32_t IPMSG_ANSREADMSG     = 0x00000032UL;

// Info
constexpr uint32_t IPMSG_GETINFO        = 0x00000040UL;
constexpr uint32_t IPMSG_SENDINFO       = 0x00000041UL;

// Absence
constexpr uint32_t IPMSG_GETABSENCEINFO = 0x00000050UL;
constexpr uint32_t IPMSG_SENDABSENCEINFO= 0x00000051UL;

// File transfer
constexpr uint32_t IPMSG_GETFILEDATA    = 0x00000060UL;
constexpr uint32_t IPMSG_RELEASEFILES   = 0x00000061UL;
constexpr uint32_t IPMSG_GETDIRFILES    = 0x00000062UL;
constexpr uint32_t IPMSG_DIRFILES_AUTH  = 0x00000063UL;
constexpr uint32_t IPMSG_DIRFILES_AUTHRET = 0x00000064UL;

// Encryption
constexpr uint32_t IPMSG_GETPUBKEY      = 0x00000072UL;
constexpr uint32_t IPMSG_ANSPUBKEY      = 0x00000073UL;

// ---------- Option Flags (High 24 bits) ----------
// General options
constexpr uint32_t IPMSG_ABSENCEOPT     = 0x00000100UL;
constexpr uint32_t IPMSG_SERVEROPT      = 0x00000200UL;
constexpr uint32_t IPMSG_DIALUPOPT      = 0x00010000UL;
constexpr uint32_t IPMSG_FILEATTACHOPT  = 0x00200000UL;
constexpr uint32_t IPMSG_ENCRYPTOPT     = 0x00400000UL;
constexpr uint32_t IPMSG_UTF8OPT        = 0x00800000UL;
constexpr uint32_t IPMSG_CAPUTF8OPT     = 0x01000000UL;
constexpr uint32_t IPMSG_ENCEXTMSGOPT   = 0x04000000UL;
constexpr uint32_t IPMSG_CLIPBOARDOPT   = 0x08000000UL;
constexpr uint32_t IPMSG_CAPFILEENCOPT  = 0x00040000UL;
constexpr uint32_t IPMSG_CAPIPDICTOPT   = 0x02000000UL;

// SENDMSG options
constexpr uint32_t IPMSG_SENDCHECKOPT   = 0x00000100UL;
constexpr uint32_t IPMSG_SECRETOPT      = 0x00000200UL;
constexpr uint32_t IPMSG_BROADCASTOPT   = 0x00000400UL;
constexpr uint32_t IPMSG_MULTICASTOPT   = 0x00000800UL;
constexpr uint32_t IPMSG_AUTORETOPT     = 0x00002000UL;
constexpr uint32_t IPMSG_RETRYOPT       = 0x00004000UL;
constexpr uint32_t IPMSG_PASSWORDOPT    = 0x00008000UL;
constexpr uint32_t IPMSG_NOLOGOPT       = 0x00020000UL;
constexpr uint32_t IPMSG_NOADDLISTOPT   = 0x00080000UL;
constexpr uint32_t IPMSG_READCHECKOPT   = 0x00100000UL;

// ---------- Buffer / Size Constants ----------
constexpr int MAX_SOCKBUF    = 8 * 1024 * 1024;  // 8 MB: FeiQ bursts the whole screenshot-fragment set (~1MB of UDP packets) within tens of ms; a small buffer overflows and drops fragments so the image can never be reassembled.
constexpr int MAX_UDPBUF     = 32 * 1024;
constexpr int MAX_NAMEBUF    = 80;
constexpr int MAX_VERBUF     = 40;
constexpr int MAX_MSGBODY    = 65536;
constexpr int MAX_BUF        = 1024;

} // namespace ipmsg
