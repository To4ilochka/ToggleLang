use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1CreateFactory, ID2D1Factory,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory, IDWriteFactory,
};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ,
    SelectObject,
};
use windows::Win32::Graphics::Imaging::{CLSID_WICImagingFactory, IWICImagingFactory};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

use crate::core::error::{AppError, AppResult};
use crate::platform::win_util::{
    BitmapGuard, CompatibleDcGuard, ScreenDcGuard, top_down_bgra_bitmap_info,
};
use crate::ui::layout::RectF;
use crate::ui::palette::Color;

/// Shared Direct2D, DirectWrite, and WIC COM factories created once per UI process
/// and cheaply cloned (COM `AddRef`) into both `FlyoutRenderer` and `TrayIconRenderer`.
#[derive(Clone)]
pub struct GraphicsFactories {
    pub d2d: ID2D1Factory,
    pub dwrite: IDWriteFactory,
    pub wic: IWICImagingFactory,
}

impl GraphicsFactories {
    pub fn new() -> AppResult<Self> {
        // SAFETY: Creating single-threaded D2D, shared DirectWrite, and WIC COM factories on UI STA.
        let d2d: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .map_err(|e| AppError::Backend(format!("D2D1CreateFactory failed: {e}")))?;
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
            .map_err(|e| AppError::Backend(format!("DWriteCreateFactory failed: {e}")))?;
        let wic: IWICImagingFactory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| {
                    AppError::Backend(format!("CoCreateInstance(WICImagingFactory) failed: {e}"))
                })?;

        Ok(Self { d2d, dwrite, wic })
    }
}

/// RAII 32-bit premultiplied BGRA backbuffer (`CreateDIBSection` + memory `HDC`).
/// All intermediate GDI handles are RAII-guarded so error paths never leak resources.
pub struct DibSurface {
    mem_dc: HDC,
    bitmap: HBITMAP,
    old_obj: HGDIOBJ,
    width_px: u32,
    height_px: u32,
}

impl DibSurface {
    pub fn new(width_px: u32, height_px: u32) -> AppResult<Self> {
        let width_px = width_px.max(1);
        let height_px = height_px.max(1);
        let bmi = top_down_bgra_bitmap_info(width_px, height_px);

        let screen_dc = ScreenDcGuard::acquire(None)
            .ok_or_else(|| AppError::Backend("GetDC(NULL) failed".into()))?;
        let mem_dc = CompatibleDcGuard::create(screen_dc.raw())
            .ok_or_else(|| AppError::Backend("CreateCompatibleDC failed".into()))?;

        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: `screen_dc` and `bmi` are valid; `bitmap` is wrapped immediately in `BitmapGuard`.
        let bitmap = BitmapGuard::new(
            unsafe { CreateDIBSection(screen_dc.raw(), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }
                .map_err(|e| AppError::Backend(format!("CreateDIBSection(flyout) failed: {e}")))?,
        );

        if bits.is_null() {
            return Err(AppError::Backend(
                "CreateDIBSection returned null pixel buffer".into(),
            ));
        }

        // SAFETY: Both `mem_dc` and `bitmap` are valid; ownership moves into `Self` after selection.
        let old_obj = unsafe { SelectObject(mem_dc.raw(), bitmap.raw()) };
        Ok(Self {
            mem_dc: mem_dc.into_raw(),
            bitmap: bitmap.into_raw(),
            old_obj,
            width_px,
            height_px,
        })
    }

    pub fn mem_dc(&self) -> HDC {
        self.mem_dc
    }

    pub fn width_px(&self) -> u32 {
        self.width_px
    }

    pub fn height_px(&self) -> u32 {
        self.height_px
    }
}

impl Drop for DibSurface {
    fn drop(&mut self) {
        // SAFETY: Restoring previous GDI object before deleting our owned bitmap and memory DC.
        unsafe {
            if !self.mem_dc.is_invalid() {
                let _ = SelectObject(self.mem_dc, self.old_obj);
                let _ = DeleteDC(self.mem_dc);
            }
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap);
            }
        }
    }
}

/// Converts a square `size x size` 32-bit premultiplied BGRA pixel slice into an owned Win32 `HICON`.
pub fn bgra_pixels_to_hicon(pixels: &[u8], size: u32) -> AppResult<HICON> {
    let expected_len = (size as usize)
        .checked_mul(size as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| AppError::Backend("Tray icon dimensions overflowed".into()))?;
    if pixels.len() != expected_len {
        return Err(AppError::Backend(format!(
            "Tray icon pixel buffer length mismatch: got {}, expected {expected_len}",
            pixels.len()
        )));
    }

    let bmi = top_down_bgra_bitmap_info(size, size);
    let screen_dc = ScreenDcGuard::acquire(None)
        .ok_or_else(|| AppError::Backend("GetDC(NULL) failed".into()))?;

    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `screen_dc` and `bmi` are valid; `color_bmp` is freed via `BitmapGuard` on drop.
    let color_bmp = BitmapGuard::new(
        unsafe { CreateDIBSection(screen_dc.raw(), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }
            .map_err(|e| AppError::Backend(format!("CreateDIBSection(icon) failed: {e}")))?,
    );

    if bits.is_null() {
        return Err(AppError::Backend(
            "CreateDIBSection(icon) returned null pixel buffer".into(),
        ));
    }

    // SAFETY: `bits` points to `size * size * 4 == pixels.len()` writable bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
    }

    // SAFETY: Creating 1-bit mask bitmap; freed via `BitmapGuard` on drop.
    let mask_bmp = BitmapGuard::new(unsafe { CreateBitmap(size as i32, size as i32, 1, 1, None) });
    if mask_bmp.raw().is_invalid() {
        return Err(AppError::Backend("CreateBitmap(icon mask) failed".into()));
    }

    let info = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask_bmp.raw(),
        hbmColor: color_bmp.raw(),
    };
    // SAFETY: `CreateIconIndirect` copies the bitmaps into an owned `HICON`; guards free our copies.
    unsafe { CreateIconIndirect(&info) }
        .map_err(|e| AppError::Backend(format!("CreateIconIndirect failed: {e}")))
}

pub fn to_d2d_color(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

pub fn to_d2d_rect(r: RectF) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}
