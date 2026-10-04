// ============================================================================
// Input validation for the network settings (broadcast segments, direct
// users, IP scan ranges). Pure functions, no React.
// ============================================================================

const OCTET = '(25[0-5]|2[0-4]\\d|1\\d\\d|[1-9]?\\d)';
const IPV4_RE = new RegExp(`^${OCTET}(\\.${OCTET}){3}$`);

export function isIPv4(s: string): boolean {
  return IPV4_RE.test(s);
}

function ipToInt(ip: string): number {
  return ip.split('.').reduce((acc, o) => (acc << 8) + Number(o), 0) >>> 0;
}

function intToIp(n: number): string {
  return [24, 16, 8, 0].map((sh) => (n >>> sh) & 255).join('.');
}

/**
 * Accepts either a broadcast address ("192.168.1.255") or a CIDR block
 * ("192.168.1.0/24", "10.8.0.0/16"). CIDR input is converted to the directed
 * broadcast address the backend expects. Returns the normalised value or an
 * error message.
 */
export function normalizeSegment(input: string): { value: string } | { error: string } {
  const s = input.trim();
  const slash = s.indexOf('/');
  if (slash < 0) {
    if (!isIPv4(s)) return { error: '请输入合法的 IPv4 广播地址，如 192.168.1.255' };
    return { value: s };
  }
  const ip = s.slice(0, slash);
  const prefix = Number(s.slice(slash + 1));
  if (!isIPv4(ip) || !Number.isInteger(prefix) || prefix < 1 || prefix > 30) {
    return { error: 'CIDR 格式应为 网络地址/前缀长度，如 192.168.1.0/24（前缀 1~30）' };
  }
  const mask = prefix === 0 ? 0 : (0xffffffff << (32 - prefix)) >>> 0;
  const broadcast = ((ipToInt(ip) & mask) | (~mask >>> 0)) >>> 0;
  return { value: intToIp(broadcast) };
}

/** "ip:port" with a valid IPv4 and port 1..65535. */
export function normalizeDirectUser(input: string): { value: string } | { error: string } {
  const s = input.trim();
  const colon = s.lastIndexOf(':');
  if (colon < 0) return { error: '格式应为 IP:端口，如 10.8.33.50:2425' };
  const ip = s.slice(0, colon);
  const port = Number(s.slice(colon + 1));
  if (!isIPv4(ip)) return { error: 'IP 地址不合法' };
  if (!Number.isInteger(port) || port < 1 || port > 65535) return { error: '端口必须在 1~65535 之间' };
  return { value: `${ip}:${port}` };
}

/**
 * "startIp-endIp" or the shorthand "startIp-lastOctet"
 * ("10.8.33.1-10.8.33.254" / "10.8.33.1-254"). The end must not precede the
 * start. Shorthand is expanded so the stored value is unambiguous.
 */
export function normalizeScanRange(input: string): { value: string } | { error: string } {
  const s = input.trim();
  const dash = s.lastIndexOf('-');
  if (dash < 0) return { error: '格式应为 起始IP-结束IP，如 10.8.33.1-254' };
  const start = s.slice(0, dash);
  let end = s.slice(dash + 1);
  if (!isIPv4(start)) return { error: '起始 IP 不合法' };
  if (/^\d{1,3}$/.test(end)) {
    const last = Number(end);
    if (last > 255) return { error: '结束段必须在 0~255 之间' };
    end = start.slice(0, start.lastIndexOf('.') + 1) + last;
  } else if (!isIPv4(end)) {
    return { error: '结束 IP 不合法' };
  }
  if (ipToInt(end) < ipToInt(start)) return { error: '结束 IP 不能小于起始 IP' };
  const count = ipToInt(end) - ipToInt(start) + 1;
  if (count > 65536) return { error: '范围过大（最多 65536 个地址）' };
  return { value: `${start}-${end}` };
}
