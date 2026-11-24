use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, FreeLibrary, HANDLE, HMODULE, HWND};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
    HBITMAP, HDC, ReleaseDC,
};
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::core::{GUID, PCWSTR};

use crate::core::error::{AppError, AppResult};

/// RAII wrapper ensuring Win32 kernel `HANDLE`s are closed on drop.
pub struct HandleGuard(pub HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: `self.0` is an owned valid Win32 handle obtained by the caller.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// RAII wrapper for a dynamically loaded System32 DLL (`HMODULE`), freeing it on drop.
pub struct LibraryGuard(HMODULE);

impl LibraryGuard {
    pub fn load_system32(dll_name: &str) -> AppResult<Self> {
        let wide = to_wide_null(dll_name);
        // SAFETY: Loads a system-trusted DLL strictly from System32.
        let module =
            unsafe { LoadLibraryExW(PCWSTR(wide.as_ptr()), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }
                .map_err(|e| AppError::Backend(format!("Failed to load {dll_name}: {e}")))?;
        Ok(Self(module))
    }

    pub fn raw(&self) -> HMODULE {
        self.0
    }
}

impl Drop for LibraryGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: `self.0` was loaded by `LoadLibraryExW` and is owned exclusively by `self`.
            unsafe {
                let _ = FreeLibrary(self.0);
            }
        }
    }
}

/// RAII wrapper for a window/screen device context acquired via `GetDC`.
pub struct ScreenDcGuard {
    hwnd: HWND,
    hdc: HDC,
}

impl ScreenDcGuard {
    pub fn acquire(hwnd: Option<HWND>) -> Option<Self> {
        let target = hwnd.unwrap_or_default();
        // SAFETY: Acquiring a device context paired with `ReleaseDC` in `Drop`.
        let hdc = unsafe { GetDC(target) };
        (!hdc.is_invalid()).then_some(Self { hwnd: target, hdc })
    }

    pub fn raw(&self) -> HDC {
        self.hdc
    }
}

impl Drop for ScreenDcGuard {
    fn drop(&mut self) {
        if !self.hdc.is_invalid() {
            // SAFETY: Paired 1-to-1 with `GetDC(self.hwnd)`.
            unsafe {
                let _ = ReleaseDC(self.hwnd, self.hdc);
            }
        }
    }
}

/// RAII wrapper for a memory device context created via `CreateCompatibleDC`.
pub struct CompatibleDcGuard(HDC);

impl CompatibleDcGuard {
    pub fn create(reference_dc: HDC) -> Option<Self> {
        // SAFETY: Creating a memory DC compatible with `reference_dc`; deleted in `Drop`.
        let mem_dc = unsafe { CreateCompatibleDC(reference_dc) };
        (!mem_dc.is_invalid()).then_some(Self(mem_dc))
    }

    pub fn raw(&self) -> HDC {
        self.0
    }

    pub fn into_raw(self) -> HDC {
        let hdc = self.0;
        std::mem::forget(self);
        hdc
    }
}

impl Drop for CompatibleDcGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: `self.0` was created via `CreateCompatibleDC` and is owned by `self`.
            unsafe {
                let _ = DeleteDC(self.0);
            }
        }
    }
}

/// RAII wrapper for an owned GDI `HBITMAP`, calling `DeleteObject` on drop unless ownership moves.
pub struct BitmapGuard(HBITMAP);

impl BitmapGuard {
    pub fn new(bitmap: HBITMAP) -> Self {
        Self(bitmap)
    }

    pub fn raw(&self) -> HBITMAP {
        self.0
    }

    pub fn into_raw(self) -> HBITMAP {
        let bmp = self.0;
        std::mem::forget(self);
        bmp
    }
}

impl Drop for BitmapGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: `self.0` is an owned GDI bitmap not currently selected into any DC.
            unsafe {
                let _ = DeleteObject(self.0);
            }
        }
    }
}

/// Single authoritative 32-bit top-down BGRA `BITMAPINFO` descriptor for GDI/Direct2D/WIC surfaces.
pub fn top_down_bgra_bitmap_info(width_px: u32, height_px: u32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width_px as i32,
            biHeight: -(height_px as i32), // negative height = top-down DIB
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Canonical per-user data directory (`%LOCALAPPDATA%\ToggleLang`), shared by state storage and crash logs.
pub fn app_data_dir() -> PathBuf {
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA")
        && !local_app_data.trim().is_empty()
    {
        return PathBuf::from(local_app_data).join("ToggleLang");
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn wide_as_bytes(wide: &[u16]) -> &[u8] {
    // SAFETY: `u16` has no padding and can always be viewed as a byte slice of length `len * 2`.
    unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), std::mem::size_of_val(wide)) }
}

pub fn wide_nul_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..len])
        .to_string_lossy()
        .into_owned()
}

pub fn utf16_bytes_to_u16s(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .collect()
}

pub fn utf16_bytes_to_string(bytes: &[u8]) -> String {
    wide_nul_to_string(&utf16_bytes_to_u16s(bytes))
}

pub fn atomic_replace_file(src: &Path, dst: &Path) -> AppResult<()> {
    let wide_src = to_wide_null(&src.to_string_lossy());
    let wide_dst = to_wide_null(&dst.to_string_lossy());
    // SAFETY: Both pointers reference NUL-terminated UTF-16 buffers valid for the call duration.
    unsafe {
        MoveFileExW(
            PCWSTR(wide_src.as_ptr()),
            PCWSTR(wide_dst.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|e| AppError::Storage(format!("MoveFileExW failed for {}: {e}", dst.display())))
}

pub fn format_guid(g: &GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7],
    )
}

pub fn parse_guid(s: &str) -> Option<GUID> {
    let trimmed = s.trim().trim_start_matches('{').trim_end_matches('}');
    let parts: Vec<&str> = trimmed.split('-').collect();
    if parts.len() != 5 {
        return None;
    }
    let data1 = u32::from_str_radix(parts[0], 16).ok()?;
    let data2 = u16::from_str_radix(parts[1], 16).ok()?;
    let data3 = u16::from_str_radix(parts[2], 16).ok()?;
    let p3 = u16::from_str_radix(parts[3], 16).ok()?;
    let p4 = u64::from_str_radix(parts[4], 16).ok()?;
    let mut data4 = [0u8; 8];
    data4[0] = (p3 >> 8) as u8;
    data4[1] = (p3 & 0xFF) as u8;
    for i in 0..6 {
        data4[2 + i] = ((p4 >> ((5 - i) * 8)) & 0xFF) as u8;
    }
    Some(GUID {
        data1,
        data2,
        data3,
        data4,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_round_trip_and_invalid_inputs() {
        let g = GUID::from_u128(0x03B5835F_F03C_411B_9CE2_AA23E1171E36);
        let formatted = format_guid(&g);
        assert_eq!(formatted, "{03B5835F-F03C-411B-9CE2-AA23E1171E36}");
        assert_eq!(parse_guid(&formatted), Some(g));
        assert_eq!(parse_guid("not-a-guid"), None);
        assert_eq!(parse_guid("{ZZZZZZZZ-F03C-411B-9CE2-AA23E1171E36}"), None);
    }

    #[test]
    fn wide_and_utf16_byte_conversions_round_trip() {
        let wide = to_wide_null("УКР-EN");
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(wide_nul_to_string(&wide), "УКР-EN");

        let bytes = wide_as_bytes(&wide);
        assert_eq!(utf16_bytes_to_string(bytes), "УКР-EN");
    }

    #[test]
    fn top_down_bitmap_info_uses_negative_height() {
        let bmi = top_down_bgra_bitmap_info(348, 260);
        assert_eq!(bmi.bmiHeader.biWidth, 348);
        assert_eq!(bmi.bmiHeader.biHeight, -260);
        assert_eq!(bmi.bmiHeader.biBitCount, 32);
    }
}
