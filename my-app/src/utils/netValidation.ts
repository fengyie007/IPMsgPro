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
  if (!s || s.length > 64) return { error: '扫描范围为空或过长' };
  let first: number, last: number;
  if (s.includes('/')) {
    const parts = s.split('/'), ip = parts[0].trim();
    if (parts.length !== 2 || !isIPv4(ip) || !/^\d{1,2}$/.test(parts[1]) || Number(parts[1]) > 32) return { error: 'CIDR 格式应为 IPv4/0～32' };
    const prefix = Number(parts[1]), size = 2 ** (32 - prefix);
    first = Math.floor(ipToInt(ip) / size) * size;
    last = first + size - 1;
    if (prefix <= 30) { first++; last--; }
  } else {
    const parts = s.split('-'), start = parts[0].trim();
    if (parts.length > 2 || !isIPv4(start)) return { error: '起始 IP 不合法' };
    let end = (parts[1] ?? start).trim();
    if (parts.length === 2 && /^\d{1,3}$/.test(end)) {
      if (Number(end) > 255) return { error: '结束段必须为0～255' };
      end = start.slice(0, start.lastIndexOf('.') + 1) + Number(end);
    }
    if (!isIPv4(end)) return { error: '结束 IP 不合法' };
    first = ipToInt(start); last = ipToInt(end);
  }
  if (first < 0x01000000 || last >= 0xe0000000 || last < first) return { error: '范围必须是正序单播 IPv4 地址，不含0网段、组播或保留地址' };
  if (last - first + 1 > 65536) return { error: '每次扫描最多65536个地址' };
  return { value: `${intToIp(first)}-${intToIp(last)}` };
}

export function validateScanOptions(ranges: readonly string[], port: number, delayMs: number):
  { value: { ranges: string[]; port: number; delayMs: number; total: number } } | { error: string } {
  if (!Number.isInteger(port) || port < 1 || port > 65535) return { error: '目标端口必须为1～65535' };
  if (!Number.isInteger(delayMs) || delayMs < 10 || delayMs > 1000) return { error: '扫描间隔必须为10～1000毫秒' };
  if (ranges.length > 32) return { error: '扫描范围最多32项' };
  const normalized: string[] = [], intervals: [number, number][] = [];
  for (const range of ranges) {
    const parsed = normalizeScanRange(range);
    if ('error' in parsed) return parsed;
    normalized.push(parsed.value);
    const [start, end] = parsed.value.split('-'); intervals.push([ipToInt(start), ipToInt(end)]);
  }
  intervals.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const merged: [number, number][] = [];
  for (const [start, end] of intervals) {
    const previous = merged[merged.length - 1];
    if (previous && start <= previous[1] + 1) previous[1] = Math.max(previous[1], end);
    else merged.push([start, end]);
  }
  const total = merged.reduce((sum, [start, end]) => sum + end - start + 1, 0);
  if (total > 65536) return { error: '合并后的扫描范围最多65536个地址' };
  return { value: { ranges: normalized, port, delayMs, total } };
}
