//! One monitor in physical pixels. All native handles stay on the capture worker.
use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder};
use ipmsg_core::image::dib::{MAX_PIXELS, MAX_SIDE};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
impl Bounds {
    pub fn validate(self) -> Result<Self, String> {
        if self.width == 0
            || self.height == 0
            || self.width > MAX_SIDE
            || self.height > MAX_SIDE
            || u64::from(self.width) * u64::from(self.height) > MAX_PIXELS
            || self.x.checked_add(self.width as i32).is_none()
            || self.y.checked_add(self.height as i32).is_none()
        {
            return Err("显示器尺寸无效或超过1600万像素截图上限".into());
        }
        Ok(self)
    }
}

fn encode_bgra(bounds: Bounds, mut pixels: Vec<u8>) -> Result<Vec<u8>, String> {
    bounds.validate()?;
    if pixels.len() != bounds.width as usize * bounds.height as usize * 4 {
        return Err("截图像素数据不完整".into());
    }
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255; // GDI's unused alpha byte is not transparency.
    }
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(
            &pixels,
            bounds.width,
            bounds.height,
            ExtendedColorType::Rgba8,
        )
        .map_err(|e| e.to_string())?;
    Ok(png)
}

#[cfg(windows)]
pub fn capture(bounds: Bounds) -> Result<Vec<u8>, String> {
    use std::{
        mem::{size_of, zeroed},
        ptr::null_mut,
    };
    use windows_sys::Win32::{
        Graphics::{Dwm::DwmFlush, Gdi::*},
        UI::HiDpi::{
            SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        },
    };
    struct Dpi(DPI_AWARENESS_CONTEXT);
    impl Drop for Dpi {
        fn drop(&mut self) {
            unsafe {
                SetThreadDpiAwarenessContext(self.0);
            }
        }
    }
    struct Handles {
        screen: HDC,
        memory: HDC,
        bitmap: HBITMAP,
        old: HGDIOBJ,
    }
    impl Drop for Handles {
        fn drop(&mut self) {
            unsafe {
                if !self.old.is_null() {
                    SelectObject(self.memory, self.old);
                }
                if !self.bitmap.is_null() {
                    DeleteObject(self.bitmap);
                }
                if !self.memory.is_null() {
                    DeleteDC(self.memory);
                }
                if !self.screen.is_null() {
                    ReleaseDC(null_mut(), self.screen);
                }
            }
        }
    }
    let bounds = bounds.validate()?;
    unsafe {
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if previous.is_null() {
            return Err("无法设置截图DPI上下文".into());
        }
        let _dpi = Dpi(previous);
        // Hide was dispatched before this worker. Wait for the desktop compositor.
        if DwmFlush() < 0 {
            return Err("等待桌面刷新失败".into());
        }
        let mut handles = Handles {
            screen: GetDC(null_mut()),
            memory: null_mut(),
            bitmap: null_mut(),
            old: null_mut(),
        };
        if handles.screen.is_null() {
            return Err("无法访问桌面".into());
        }
        handles.memory = CreateCompatibleDC(handles.screen);
        handles.bitmap =
            CreateCompatibleBitmap(handles.screen, bounds.width as i32, bounds.height as i32);
        if handles.memory.is_null() || handles.bitmap.is_null() {
            return Err("无法分配截图缓冲区".into());
        }
        handles.old = SelectObject(handles.memory, handles.bitmap);
        if handles.old.is_null() || handles.old as isize == -1 {
            handles.old = null_mut();
            return Err("无法选择截图位图".into());
        }
        if BitBlt(
            handles.memory,
            0,
            0,
            bounds.width as i32,
            bounds.height as i32,
            handles.screen,
            bounds.x,
            bounds.y,
            SRCCOPY | CAPTUREBLT,
        ) == 0
        {
            return Err("桌面截图失败".into());
        }
        SelectObject(handles.memory, handles.old);
        handles.old = null_mut();
        let mut info: BITMAPINFO = zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = bounds.width as i32;
        info.bmiHeader.biHeight = -(bounds.height as i32); // top-down, no vertical flip
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut pixels = vec![0; bounds.width as usize * bounds.height as usize * 4];
        if GetDIBits(
            handles.memory,
            handles.bitmap,
            0,
            bounds.height,
            pixels.as_mut_ptr().cast(),
            &mut info,
            DIB_RGB_COLORS,
        ) != bounds.height as i32
        {
            return Err("读取截图像素失败".into());
        }
        drop(handles);
        encode_bgra(bounds, pixels)
    }
}

#[cfg(not(windows))]
pub fn capture(_: Bounds) -> Result<Vec<u8>, String> {
    Err("当前平台暂不支持截图".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn negative_monitor_coordinates_and_size_limits() {
        assert!(Bounds {
            x: -3840,
            y: -100,
            width: 3840,
            height: 2160
        }
        .validate()
        .is_ok());
        for (width, height) in [(0, 10), (10, 0), (8000, 4000), (MAX_SIDE + 1, 1)] {
            assert!(Bounds {
                x: 0,
                y: 0,
                width,
                height
            }
            .validate()
            .is_err());
        }
        assert!(Bounds {
            x: i32::MAX,
            y: 0,
            width: 2,
            height: 2
        }
        .validate()
        .is_err());
    }
    #[test]
    fn native_bgra_is_opaque_top_down_png() {
        let bounds = Bounds {
            x: -1,
            y: 0,
            width: 1,
            height: 2,
        };
        let png = encode_bgra(bounds, vec![0, 0, 255, 0, 255, 0, 0, 123]).unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(image.into_raw(), vec![255, 0, 0, 255, 0, 0, 255, 255]);
        assert!(encode_bgra(bounds, vec![0; 4]).is_err());
    }
}
