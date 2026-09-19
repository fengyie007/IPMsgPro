// ============================================================================
// Screen capture: BitBlt one monitor into a DIB and encode it as PNG via GDI+.
// ============================================================================
#include "bridge/screen_capture.h"

#include <objidl.h>
#include <gdiplus.h>

#include <cstring>

using namespace Gdiplus;

namespace ipmsg {


// Find the PNG image encoder CLSID.
static bool GetPngEncoderClsid(CLSID* clsid) {
    UINT num = 0, size = 0;
    GetImageEncodersSize(&num, &size);
    if (size == 0) return false;
    std::vector<ImageCodecInfo> encoders(size / sizeof(ImageCodecInfo));
    GetImageEncoders(num, size, encoders.data());
    for (UINT i = 0; i < num; i++) {
        if (wcscmp(encoders[i].MimeType, L"image/png") == 0) {
            *clsid = encoders[i].Clsid;
            return true;
        }
    }
    return false;
}

// Capture a specific monitor to a PNG, returning the raw encoded bytes.
bool CaptureMonitorToPng(HMONITOR hMonitor, std::vector<unsigned char>& outPng) {
    MONITORINFO mi = { sizeof(mi) };
    if (!GetMonitorInfo(hMonitor, &mi)) return false;
    int left = mi.rcMonitor.left, top = mi.rcMonitor.top;
    int w = mi.rcMonitor.right - left;
    int h = mi.rcMonitor.bottom - top;
    if (w <= 0 || h <= 0) return false;

    HDC hdcScreen = GetDC(NULL);
    if (!hdcScreen) return false;

    // Create a 32bpp DIB section so we get reliable pixel access.
    BITMAPINFOHEADER bi = { 0 };
    bi.biSize = sizeof(BITMAPINFOHEADER);
    bi.biWidth = w;
    bi.biHeight = h;          // positive => bottom-up DIB
    bi.biPlanes = 1;
    bi.biBitCount = 32;
    bi.biCompression = BI_RGB;
    void* pBits = nullptr;
    HBITMAP hBmp = CreateDIBSection(hdcScreen, (BITMAPINFO*)&bi, DIB_RGB_COLORS, &pBits, NULL, 0);
    if (!hBmp) {
        ReleaseDC(NULL, hdcScreen);
        return false;
    }

    HDC hdcMem = CreateCompatibleDC(hdcScreen);
    HGDIOBJ old = SelectObject(hdcMem, hBmp);
    BitBlt(hdcMem, 0, 0, w, h, hdcScreen, left, top, SRCCOPY | CAPTUREBLT);
    SelectObject(hdcMem, old);

    // Encode via GDI+.
    GdiplusStartupInput gdiplusStartupInput;
    ULONG_PTR gdiplusToken = 0;
    if (GdiplusStartup(&gdiplusToken, &gdiplusStartupInput, NULL) != Ok) {
        DeleteObject(hBmp);
        DeleteDC(hdcMem);
        ReleaseDC(NULL, hdcScreen);
        return false;
    }

    bool ok = false;
    Bitmap* bmp = new Bitmap(w, h, PixelFormat32bppARGB);
    {
        BitmapData bmpData;
        Rect rect(0, 0, w, h);
        if (bmp->LockBits(&rect, ImageLockModeWrite, PixelFormat32bppARGB, &bmpData) == Ok) {
            int dstStride = bmpData.Stride;
            BYTE* dst = (BYTE*)bmpData.Scan0;
            int srcStride = ((w * 32 + 31) / 32) * 4;
            for (int y = 0; y < h; y++) {
                // DIB is bottom-up: row 0 is the bottom row of the image.
                BYTE* srcRow = (BYTE*)pBits + (h - 1 - y) * srcStride;
                BYTE* dstRow = dst + y * dstStride;
                for (int x = 0; x < w; x++) {
                    BYTE b = srcRow[x * 4 + 0];
                    BYTE g = srcRow[x * 4 + 1];
                    BYTE r = srcRow[x * 4 + 2];
                    BYTE a = srcRow[x * 4 + 3];
                    DWORD pix = ((a ? a : 0xFF) << 24) | (r << 16) | (g << 8) | b;
                    *(DWORD*)(dstRow + x * 4) = pix;
                }
            }
            bmp->UnlockBits(&bmpData);
        }

        CLSID pngClsid;
        if (GetPngEncoderClsid(&pngClsid)) {
            IStream* pStream = NULL;
            if (CreateStreamOnHGlobal(NULL, TRUE, &pStream) == S_OK) {
                if (bmp->Save(pStream, &pngClsid, NULL) == Ok) {
                    HGLOBAL hMem = NULL;
                    if (GetHGlobalFromStream(pStream, &hMem) == S_OK) {
                        SIZE_T size = GlobalSize(hMem);
                        void* pData = GlobalLock(hMem);
                        if (pData) {
                            outPng.assign((BYTE*)pData, (BYTE*)pData + size);
                            ok = true;
                            GlobalUnlock(hMem);
                        }
                    }
                }
                pStream->Release();
            }
        }
    }

    delete bmp;
    GdiplusShutdown(gdiplusToken);
    DeleteObject(hBmp);
    DeleteDC(hdcMem);
    ReleaseDC(NULL, hdcScreen);
    return ok;
}

}  // namespace ipmsg
