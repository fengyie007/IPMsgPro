import type { Config } from '../types';
import { normalizeDirectUser, normalizeScanRange, validateScanOptions } from './netValidation';

export function settingsDirty(saved: Config, draft: Config, directInput = '', scanInput = ''): boolean {
  return JSON.stringify(saved) !== JSON.stringify(draft) || !!directInput.trim() || !!scanInput.trim();
}

export function prepareSettings(draft: Config, directInput: string, scanInput: string): Config {
  const next = { ...draft, directUsers: [...draft.directUsers], ipScanRanges: [...draft.ipScanRanges] };
  if (directInput.trim()) {
    const direct = normalizeDirectUser(directInput);
    if ('error' in direct) throw new Error(direct.error);
    if (!next.directUsers.includes(direct.value)) next.directUsers.push(direct.value);
  }
  if (scanInput.trim()) {
    const range = normalizeScanRange(scanInput);
    if ('error' in range) throw new Error(range.error);
    if (!next.ipScanRanges.includes(range.value)) next.ipScanRanges.push(range.value);
  }
  const scan = validateScanOptions(next.ipScanRanges, next.scanPort, next.scanDelayMs);
  if ('error' in scan) throw new Error(scan.error);
  return { ...next, ipScanRanges: scan.value.ranges };
}
