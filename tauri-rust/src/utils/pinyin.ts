// ============================================================================
// Pinyin initials for contact search (e.g. "zs" matches 张三)
// ============================================================================

// Chromium's zh collation orders Hanzi by pinyin, so comparing a character against
// the first character of each initial gives its initial without bundling a table.
const BOUNDARIES = [...'阿八嚓哒妸发旮哈讥咔垃妈拏噢妑七呥扨它穵夕丫帀'];
const LETTERS = [...'abcdefghjklmnopqrstwxyz'];
const collator = new Intl.Collator('zh-CN');

// shortcut: collation uses the most common reading, so polyphonic surnames are
// overridden by hand. Add entries here when a real contact is misdetected.
const SURNAMES: Record<string, string> = {
  曾: 'z', 覃: 'q', 单: 's', 查: 'z', 解: 'x', 仇: 'q', 区: 'o', 乐: 'y',
  朴: 'p', 尉: 'y', 缪: 'm', 秘: 'b', 盖: 'g', 种: 'c', 薄: 'b', 柏: 'b', 长: 'c',
};

const cache = new Map<string, string>();

/** Lowercase initials of a display name; non-Hanzi characters are kept as-is. */
export function pinyinInitials(text: string): string {
  const hit = cache.get(text);
  if (hit !== undefined) return hit;
  let out = '';
  for (const [index, char] of [...text].entries()) {
    if (!/[\u4e00-\u9fa5]/.test(char)) {
      out += char.toLowerCase();
    } else if (index === 0 && SURNAMES[char]) {
      out += SURNAMES[char];
    } else {
      let letter = char;
      for (let i = BOUNDARIES.length - 1; i >= 0; i--) {
        if (collator.compare(char, BOUNDARIES[i]) >= 0) {
          letter = LETTERS[i];
          break;
        }
      }
      out += letter;
    }
  }
  if (cache.size > 500) cache.clear();
  cache.set(text, out);
  return out;
}
