//! FeiQ's LZW variant: LSB-first code bits packed into MSB-first bytes.
//! Slots 256..4095 cycle after saturation; the code width stays at 12 bits.

pub const MAX_DIB_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                0xedb88320 ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}
const CRC_TABLE: [u32; 256] = crc_table();

pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc = CRC_TABLE[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ u32::MAX
}

struct Codes<'a> {
    input: &'a [u8],
    bit: usize,
}
impl Codes<'_> {
    fn next(&mut self, width: u32) -> Option<usize> {
        if self.input.len() * 8 - self.bit < width as usize {
            return None;
        }
        let mut code = 0;
        for position in 0..width {
            let value = (self.input[self.bit / 8] >> (7 - self.bit % 8)) & 1;
            code |= usize::from(value) << position;
            self.bit += 1;
        }
        Some(code)
    }
}

/// Decode a raw stream without the 12-byte `LZW!` container header.
/// No partial output is accepted: the declared output length must match exactly.
pub fn decompress(input: &[u8], expected_len: usize) -> Result<Vec<u8>, String> {
    if input.is_empty() || input.len() > MAX_PAYLOAD_BYTES {
        return Err("LZW压缩流为空或超过大小限制".into());
    }
    if expected_len == 0 || expected_len > MAX_DIB_BYTES {
        return Err("LZW声明的DIB长度无效或超过64 MiB".into());
    }
    let mut codes = Codes { input, bit: 0 };
    let first = codes.next(9).ok_or("LZW首码被截断")?;
    if first > 255 {
        return Err("LZW首码不是字面值".into());
    }
    let mut dictionary: Vec<Vec<u8>> = (0..=255).map(|value| vec![value]).collect();
    dictionary.reserve(4096 - dictionary.len());
    let mut previous = vec![first as u8];
    let mut output = Vec::new();
    output
        .try_reserve_exact(expected_len)
        .map_err(|_| "无法分配图片解码缓冲区")?;
    output.push(first as u8);
    let mut next = 256;
    let mut width = 9;
    // The first code was already consumed; this counter does not wrap with slots.
    let mut counter = 257;
    while let Some(code) = codes.next(width) {
        let entry = if code == next {
            // A recycled slot still contains old data. KwKwK must take precedence.
            let mut entry = previous.clone();
            entry.push(previous[0]);
            entry
        } else {
            dictionary.get(code).ok_or("LZW引用未定义词条")?.clone()
        };
        if entry.len() > expected_len - output.len() {
            return Err("LZW解码结果超过声明长度".into());
        }
        output.extend_from_slice(&entry);
        let mut word = previous;
        word.push(entry[0]);
        if next == dictionary.len() {
            dictionary.push(word);
        } else {
            dictionary[next] = word;
        }
        next += 1;
        if next == 4096 {
            next = 256;
        }
        previous = entry;
        if width < 12 {
            counter += 1;
            if counter == 1 << width {
                width += 1;
            }
        }
    }
    if output.len() != expected_len {
        return Err("LZW解码结果短于声明长度".into());
    }
    Ok(output)
}

/// Encode the same cyclic dictionary variant as `decompress`.
pub fn compress(input: &[u8]) -> Result<Vec<u8>, String> {
    use std::collections::HashMap;
    if input.is_empty() || input.len() > MAX_DIB_BYTES {
        return Err("DIB为空或超过64 MiB".into());
    }
    let mut slots = vec![Vec::new(); 4096];
    let mut dictionary = HashMap::<Vec<u8>, usize>::new();
    for code in 0..256 {
        slots[code] = vec![code as u8];
        dictionary.insert(slots[code].clone(), code);
    }
    let (mut next, mut width, mut counter) = (256usize, 9u32, 256usize);
    let (mut byte, mut used) = (0u8, 0u32);
    let mut output = Vec::new();
    let mut emit = |code: usize| -> Result<(), String> {
        for bit in 0..width {
            byte = (byte << 1) | (((code >> bit) & 1) as u8);
            used += 1;
            if used == 8 {
                output.push(byte);
                byte = 0;
                used = 0;
            }
        }
        if output.len() > MAX_PAYLOAD_BYTES - 12 {
            return Err("压缩后的图片超过16 MiB".into());
        }
        if width < 12 {
            counter += 1;
            if counter == 1 << width {
                width += 1;
            }
        }
        Ok(())
    };
    let mut prefix = vec![input[0]];
    let mut prefix_code = usize::from(input[0]);
    for &value in &input[1..] {
        prefix.push(value);
        if let Some(&code) = dictionary.get(prefix.as_slice()) {
            prefix_code = code;
            continue;
        }
        emit(prefix_code)?;
        if dictionary.get(slots[next].as_slice()) == Some(&next) {
            dictionary.remove(slots[next].as_slice());
        }
        slots[next] = prefix.clone();
        dictionary.insert(prefix.clone(), next);
        next += 1;
        if next == 4096 {
            next = 256;
        }
        prefix.clear();
        prefix.push(value);
        prefix_code = usize::from(value);
    }
    emit(prefix_code)?;
    drop(emit);
    if used > 0 {
        output.push(byte << (8 - used));
    }
    if output.len() > MAX_PAYLOAD_BYTES - 12 {
        return Err("压缩后的图片超过16 MiB".into());
    }
    Ok(output)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn compressor_matches_known_codes_and_cycles_against_validated_decoder() {
        assert_eq!(compress(b"ABC").unwrap(), pack(&[65, 66, 67]));
        assert_eq!(compress(b"AAAAAA").unwrap(), pack(&[65, 256, 257]));
        let mut seed = 123456789u32;
        for size in [1, 255, 256, 1024, 4096, 100000] {
            let data: Vec<_> = (0..size)
                .map(|_| {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    (seed >> 24) as u8
                })
                .collect();
            let encoded = compress(&data).unwrap();
            assert_eq!(decompress(&encoded, data.len()).unwrap(), data);
        }
        for data in [vec![0; 100000], b"ABABABA".repeat(20000)] {
            let encoded = compress(&data).unwrap();
            assert_eq!(decompress(&encoded, data.len()).unwrap(), data);
        }
        assert!(compress(&[]).is_err());
    }

    // Independent fixture packer uses explicit ordinal boundaries, not the
    // decoder's counter. These streams need not be optimal compressor output.
    pub(crate) fn pack(codes: &[usize]) -> Vec<u8> {
        let mut result = vec![];
        let mut byte = 0u8;
        let mut used = 0;
        for (index, &code) in codes.iter().enumerate() {
            let width = match index {
                0..=255 => 9,
                256..=767 => 10,
                768..=1791 => 11,
                _ => 12,
            };
            assert!(code < 1 << width);
            for bit in 0..width {
                byte = (byte << 1) | ((code >> bit) & 1) as u8;
                used += 1;
                if used == 8 {
                    result.push(byte);
                    byte = 0;
                    used = 0;
                }
            }
        }
        if used != 0 {
            result.push(byte << (8 - used));
        }
        result
    }
    #[test]
    fn standard_crc_vectors() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
    }
    #[test]
    fn literal_codes_cross_every_width_boundary() {
        for len in [1, 255, 256, 257, 767, 768, 769, 1791, 1792, 1793, 10000] {
            let expected: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let codes: Vec<usize> = expected.iter().map(|&v| usize::from(v)).collect();
            assert_eq!(decompress(&pack(&codes), expected.len()).unwrap(), expected);
        }
    }
    #[test]
    fn cyclic_dictionary_replaces_old_entries() {
        let mut codes: Vec<usize> = (0..4097).map(|i| i % 251).collect();
        let mut expected: Vec<u8> = codes.iter().map(|&v| v as u8).collect();
        codes.push(256);
        expected.extend_from_slice(&[(3840 % 251) as u8, (3841 % 251) as u8]);
        assert_eq!(decompress(&pack(&codes), expected.len()).unwrap(), expected);
    }
    #[test]
    fn kwkwk_takes_precedence_after_wrap() {
        let mut codes: Vec<usize> = (0..3841).map(|i| i % 251).collect();
        let mut expected: Vec<u8> = codes.iter().map(|&v| v as u8).collect();
        codes.push(256);
        expected.extend_from_slice(&[(3840 % 251) as u8; 2]);
        assert_eq!(decompress(&pack(&codes), expected.len()).unwrap(), expected);
        assert_eq!(
            decompress(&pack(&[65, 66, 256, 258]), 7).unwrap(),
            b"ABABABA"
        );
    }
    #[test]
    fn malformed_or_wrong_sized_streams_are_rejected() {
        assert!(decompress(&[], 40).is_err());
        assert!(decompress(&[0], 40).is_err());
        assert!(decompress(&pack(&[300]), 1).is_err());
        assert!(decompress(&pack(&[65, 400]), 2).is_err());
        assert!(decompress(&pack(&[65]), 0).is_err());
        assert!(decompress(&pack(&[65]), MAX_DIB_BYTES + 1).is_err());
        assert!(decompress(&pack(&[65]), 2).is_err());
        assert!(decompress(&pack(&[65, 66]), 1).is_err());
    }
}
