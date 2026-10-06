//! Validate received image payloads before converting them into managed PNG assets.
//! FeiQ's LZW container contains a DIB, not a complete BMP file.

use super::lzw::{crc32, decompress, MAX_DIB_BYTES, MAX_PAYLOAD_BYTES};
use image::{
    codecs::png::PngEncoder, DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader,
    Limits, RgbImage,
};
use std::io::{self, Cursor, Write};

pub const MAX_PIXELS: u64 = 16_000_000;
pub const MAX_SIDE: u32 = 16_384;
pub const MAX_PNG_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_THUMBNAIL_BYTES: usize = 2 * 1024 * 1024;
const THUMB_WIDTH: u32 = 320;
const THUMB_HEIGHT: u32 = 240;

pub struct DecodedImage {
    pub png: Vec<u8>,
    pub thumbnail_png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
impl std::fmt::Debug for DecodedImage {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("DecodedImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("png_bytes", &self.png.len())
            .field("thumbnail_bytes", &self.thumbnail_png.len())
            .finish()
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, String> {
    let field: [u8; 2] = bytes
        .get(at..at + 2)
        .ok_or("图片头被截断")?
        .try_into()
        .map_err(|_| "图片头被截断")?;
    Ok(u16::from_le_bytes(field))
}
fn u32_at(bytes: &[u8], at: usize) -> Result<u32, String> {
    let field: [u8; 4] = bytes
        .get(at..at + 4)
        .ok_or("图片头被截断")?
        .try_into()
        .map_err(|_| "图片头被截断")?;
    Ok(u32::from_le_bytes(field))
}
fn dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0
        || height == 0
        || width > MAX_SIDE
        || height > MAX_SIDE
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err("图片尺寸为空或超过1600万像素限制".into());
    }
    Ok(())
}

fn decode_dib(dib: &[u8]) -> Result<DynamicImage, String> {
    if dib.len() < 40 || dib.len() > MAX_DIB_BYTES {
        return Err("DIB长度无效".into());
    }
    let header = u32_at(dib, 0)? as usize;
    if !matches!(header, 40 | 52 | 56 | 108 | 124) || header > dib.len() {
        return Err("不支持或截断的DIB头".into());
    }
    let signed_width = u32_at(dib, 4)? as i32;
    let signed_height = u32_at(dib, 8)? as i32;
    if signed_width <= 0 || signed_height == 0 {
        return Err("DIB宽高无效".into());
    }
    let width = signed_width as u32;
    // abs on i32::MIN would overflow; widen before taking the absolute value.
    let height64 = i64::from(signed_height).unsigned_abs();
    if height64 > u64::from(MAX_SIDE) {
        return Err("DIB高度超过限制".into());
    }
    let height = height64 as u32;
    dimensions(width, height)?;
    let bits = u16_at(dib, 14)?;
    if u16_at(dib, 12)? != 1 || !matches!(bits, 24 | 32) {
        return Err("仅支持单平面24/32位DIB".into());
    }
    if u32_at(dib, 16)? != 0 {
        return Err("暂不支持压缩或位掩码DIB".into());
    }
    if u32_at(dib, 32)? != 0 {
        return Err("暂不支持带颜色表的DIB".into());
    }
    let stride = ((u64::from(width) * u64::from(bits) + 31) / 32) * 4;
    let pixels = stride.checked_mul(height64).ok_or("DIB像素长度溢出")?;
    let expected = (header as u64).checked_add(pixels).ok_or("DIB长度溢出")?;
    if expected != dib.len() as u64 {
        return Err("DIB像素范围与数据长度不一致".into());
    }
    let size_image = u32_at(dib, 20)?;
    if size_image != 0 && u64::from(size_image) != pixels {
        return Err("DIB声明的像素长度不一致".into());
    }

    let rgb_len =
        usize::try_from(u64::from(width) * height64 * 3).map_err(|_| "DIB像素长度溢出")?;
    let mut rgb = Vec::new();
    rgb.try_reserve_exact(rgb_len)
        .map_err(|_| "无法分配图片像素缓冲区")?;
    rgb.resize(rgb_len, 0);
    let bytes_per_pixel = usize::from(bits / 8);
    for y in 0..height as usize {
        let source_y = if signed_height > 0 {
            height as usize - 1 - y
        } else {
            y
        };
        let source = header + source_y * stride as usize;
        let target = y * width as usize * 3;
        for x in 0..width as usize {
            let from = source + x * bytes_per_pixel;
            let to = target + x * 3;
            rgb[to] = dib[from + 2];
            rgb[to + 1] = dib[from + 1];
            rgb[to + 2] = dib[from];
            // BI_RGB's fourth byte is unused, often zero; never turn it into alpha.
        }
    }
    RgbImage::from_raw(width, height, rgb)
        .map(DynamicImage::ImageRgb8)
        .ok_or_else(|| "DIB像素布局无效".into())
}

fn decode_file(payload: &[u8], format: ImageFormat) -> Result<DynamicImage, String> {
    let mut reader = ImageReader::with_format(Cursor::new(payload), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DIB_BYTES as u64);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| format!("无法读取图片：{e}"))?;
    let (width, height) = decoder.dimensions();
    dimensions(width, height)?;
    if decoder.total_bytes() > MAX_DIB_BYTES as u64 {
        return Err("图片解码像素超过64 MiB".into());
    }
    let orientation = decoder
        .orientation()
        .map_err(|e| format!("图片方向信息无效：{e}"))?;
    let mut image =
        DynamicImage::from_decoder(decoder).map_err(|e| format!("图片解码失败：{e}"))?;
    image.apply_orientation(orientation);
    Ok(image)
}

struct LimitedBytes {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for LimitedBytes {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.len() > self.limit - self.bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "图片编码结果超过大小限制",
            ));
        }
        self.bytes
            .try_reserve(data.len())
            .map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "无法分配PNG缓冲区"))?;
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn png(image: &DynamicImage, limit: usize) -> Result<Vec<u8>, String> {
    let mut output = LimitedBytes {
        bytes: vec![],
        limit,
    };
    PngEncoder::new(&mut output)
        .write_image(
            image.as_bytes(),
            image.width(),
            image.height(),
            image.color().into(),
        )
        .map_err(|e| format!("PNG编码失败：{e}"))?;
    Ok(output.bytes)
}

/// Decode one fully assembled payload; never crop/pad a failed LZW/DIB decode.
/// All returned image bytes are PNGs generated from validated pixel data.
pub fn decode_image(payload: &[u8]) -> Result<DecodedImage, String> {
    if payload.is_empty() || payload.len() > MAX_PAYLOAD_BYTES {
        return Err("图片传输载荷为空或超过16 MiB".into());
    }
    let image = if payload.starts_with(b"LZW!") {
        if payload.len() < 12 {
            return Err("LZW图片头被截断".into());
        }
        let expected = u32_at(payload, 4)? as usize;
        let crc = u32_at(payload, 8)?;
        let dib = decompress(&payload[12..], expected)?;
        if crc32(&dib) != crc {
            return Err("图片CRC32校验失败".into());
        }
        decode_dib(&dib)?
    } else if payload.starts_with(b"\x89PNG\r\n\x1a\n") {
        decode_file(payload, ImageFormat::Png)?
    } else if payload.starts_with(b"\xff\xd8\xff") {
        decode_file(payload, ImageFormat::Jpeg)?
    } else {
        return Err("不支持的图片载荷格式".into());
    };
    normalize(image)
}

fn normalize(image: DynamicImage) -> Result<DecodedImage, String> {
    let width = image.width();
    let height = image.height();
    dimensions(width, height)?;
    let thumbnail = image.thumbnail(THUMB_WIDTH, THUMB_HEIGHT);
    let thumbnail_png = png(&thumbnail, MAX_THUMBNAIL_BYTES)?;
    let png = png(&image, MAX_PNG_BYTES)?;
    Ok(DecodedImage {
        png,
        thumbnail_png,
        width,
        height,
    })
}

pub const MAX_IMPORT_BYTES: usize = 20 * 1024 * 1024;

pub fn import_image(bytes: &[u8]) -> Result<DecodedImage, String> {
    if bytes.is_empty() || bytes.len() > MAX_IMPORT_BYTES {
        return Err("原图片为空或超过20 MiB".into());
    }
    let format = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        ImageFormat::Png
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        ImageFormat::Jpeg
    } else if bytes.starts_with(b"BM") {
        ImageFormat::Bmp
    } else {
        return Err("仅支持PNG、JPEG和BMP图片".into());
    };
    normalize(decode_file(bytes, format)?)
}

/// Build a FeiQ LZW! payload from an already registered PNG asset.
pub fn encode_png_for_wire(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_PNG_BYTES || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("发送资产必须是受限PNG".into());
    }
    let rgba = decode_file(bytes, ImageFormat::Png)?.to_rgba8();
    let (width, height) = rgba.dimensions();
    dimensions(width, height)?;
    let stride = (u64::from(width) * 3 + 3) & !3;
    let length = 40 + stride * u64::from(height);
    if length > MAX_DIB_BYTES as u64 {
        return Err("发送DIB超过上限".into());
    }
    let mut dib = Vec::new();
    dib.try_reserve_exact(length as usize)
        .map_err(|_| "无法分配发送DIB")?;
    for n in [40, width, height] {
        dib.extend_from_slice(&n.to_le_bytes());
    }
    dib.extend_from_slice(&[1, 0, 24, 0]);
    for n in [0, (stride * u64::from(height)) as u32, 0, 0, 0, 0] {
        dib.extend_from_slice(&n.to_le_bytes());
    }
    dib.resize(length as usize, 0);
    for y in 0..height {
        for x in 0..width {
            let pixel = rgba.get_pixel(x, y).0;
            let alpha = u16::from(pixel[3]);
            let at = 40 + (height - 1 - y) as usize * stride as usize + x as usize * 3;
            for (i, channel) in [pixel[2], pixel[1], pixel[0]].into_iter().enumerate() {
                dib[at + i] =
                    ((u16::from(channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
            }
        }
    }
    drop(rgba);
    let compressed = super::lzw::compress(&dib)?;
    if decompress(&compressed, dib.len())? != dib {
        return Err("发送图片LZW校验失败".into());
    }
    let mut payload = b"LZW!".to_vec();
    payload.extend_from_slice(&(dib.len() as u32).to_le_bytes());
    payload.extend_from_slice(&crc32(&dib).to_le_bytes());
    payload.extend_from_slice(&compressed);
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_png_is_white_composited_and_bottom_up() {
        let rgba = image::RgbaImage::from_raw(1, 2, vec![255, 0, 0, 0, 0, 0, 255, 128]).unwrap();
        let bytes = png(&DynamicImage::ImageRgba8(rgba), MAX_PNG_BYTES).unwrap();
        let wire = encode_png_for_wire(&bytes).unwrap();
        assert!(wire.starts_with(b"LZW!"));
        let result = decode_image(&wire).unwrap();
        let pixels = image::load_from_memory(&result.png).unwrap().to_rgb8();
        assert_eq!(pixels.as_raw(), &[255, 255, 255, 127, 127, 255]);
    }

    #[test]
    fn imports_bmp_and_rejects_non_images() {
        let original = DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap(),
        );
        let mut bmp = Cursor::new(Vec::new());
        original.write_to(&mut bmp, ImageFormat::Bmp).unwrap();
        let imported = import_image(bmp.get_ref()).unwrap();
        assert_eq!(
            image::load_from_memory(&imported.png).unwrap().to_rgb8(),
            original.to_rgb8()
        );
        assert!(import_image(b"GIF89a").is_err());
        assert!(import_image(b"not an image").is_err());
        assert!(encode_png_for_wire(bmp.get_ref()).is_err());
    }
    use crate::image::lzw::tests::pack;

    fn make_dib(width: i32, height: i32, bits: u16, pixels: &[u8]) -> Vec<u8> {
        let mut dib = vec![0; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&width.to_le_bytes());
        dib[8..12].copy_from_slice(&height.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&bits.to_le_bytes());
        dib[20..24].copy_from_slice(&(pixels.len() as u32).to_le_bytes());
        dib.extend_from_slice(pixels);
        dib
    }
    fn wrap(dib: &[u8]) -> Vec<u8> {
        let mut payload = b"LZW!".to_vec();
        payload.extend_from_slice(&(dib.len() as u32).to_le_bytes());
        payload.extend_from_slice(&crc32(dib).to_le_bytes());
        payload.extend_from_slice(&pack(
            &dib.iter().map(|&v| usize::from(v)).collect::<Vec<_>>(),
        ));
        payload
    }
    #[test]
    fn bottom_up_dib_and_row_padding_are_correct() {
        let dib = make_dib(1, 2, 24, &[0, 0, 255, 77, 0, 255, 0, 99]);
        let decoded = decode_image(&wrap(&dib)).unwrap();
        assert_eq!((decoded.width, decoded.height), (1, 2));
        let rgb = image::load_from_memory(&decoded.png).unwrap().to_rgb8();
        assert_eq!(rgb.as_raw(), &[0, 255, 0, 255, 0, 0]);
    }
    #[test]
    fn top_down_and_32bit_unused_alpha_are_correct() {
        let dib = make_dib(1, -2, 32, &[0, 0, 255, 0, 255, 0, 0, 0]);
        let decoded = decode_image(&wrap(&dib)).unwrap();
        let rgba = image::load_from_memory(&decoded.png).unwrap().to_rgba8();
        assert_eq!(rgba.as_raw(), &[255, 0, 0, 255, 0, 0, 255, 255]);
    }
    #[test]
    fn corrupted_container_and_dib_are_rejected() {
        let valid = make_dib(1, 1, 24, &[0, 0, 255, 0]);
        let mut bad_crc = wrap(&valid);
        bad_crc[8] ^= 1;
        assert!(decode_image(&bad_crc).is_err());
        for end in 0..12 {
            assert!(decode_image(&bad_crc[..end]).is_err());
        }
        let mut short = wrap(&valid);
        short.pop();
        assert!(decode_image(&short).is_err());
        for (at, value) in [
            (0, 256u32),
            (4, 0),
            (4, i32::MAX as u32),
            (8, 0),
            (8, i32::MIN as u32),
            (16, 3),
            (20, 100),
            (32, 1),
        ] {
            let mut dib = valid.clone();
            dib[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(decode_dib(&dib).is_err(), "accepted invalid field at {at}");
        }
        let mut extra = valid.clone();
        extra.push(0);
        assert!(decode_dib(&extra).is_err());
        assert!(decode_dib(&valid[..valid.len() - 1]).is_err());
        let mut palette = valid.clone();
        palette[14..16].copy_from_slice(&8u16.to_le_bytes());
        assert!(decode_dib(&palette).is_err());
        let mut no_planes = valid.clone();
        no_planes[12..14].copy_from_slice(&0u16.to_le_bytes());
        assert!(decode_dib(&no_planes).is_err());
    }
    #[test]
    fn zero_size_image_is_valid_for_uncompressed_dib() {
        let mut dib = make_dib(1, 1, 24, &[0, 0, 255, 0]);
        dib[20..24].fill(0);
        assert!(decode_image(&wrap(&dib)).is_ok());
    }
    #[test]
    fn raw_png_preserves_pixels_and_thumbnail_is_bounded() {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(640, 480, |x, y| {
            image::Rgba([x as u8, y as u8, 64, 128])
        }));
        let raw = png(&image, MAX_PNG_BYTES).unwrap();
        let decoded = decode_image(&raw).unwrap();
        assert_eq!(
            image::load_from_memory(&decoded.png).unwrap().to_rgba8(),
            image.to_rgba8()
        );
        let thumbnail = image::load_from_memory(&decoded.thumbnail_png).unwrap();
        assert!(thumbnail.width() <= THUMB_WIDTH && thumbnail.height() <= THUMB_HEIGHT);
    }
    #[test]
    fn raw_jpeg_is_decoded_but_unknown_formats_are_not() {
        let image = RgbImage::from_pixel(4, 3, image::Rgb([10, 20, 30]));
        let mut bytes = vec![];
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 95)
            .encode_image(&image)
            .unwrap();
        let decoded = decode_image(&bytes).unwrap();
        assert_eq!((decoded.width, decoded.height), (4, 3));
        assert!(decode_image(b"GIF89a").is_err());
        assert!(decode_image(b"BMfake").is_err());
    }
    #[test]
    fn png_output_writer_enforces_its_cap() {
        let mut writer = LimitedBytes {
            bytes: vec![],
            limit: 2,
        };
        writer.write_all(&[1, 2]).unwrap();
        assert!(writer.write_all(&[3]).is_err());
        assert_eq!(writer.bytes, [1, 2]);
    }
}
