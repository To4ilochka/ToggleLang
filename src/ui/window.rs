//! Win32 flyout window lifecycle (RAII `WindowGuard`) and monitor/DPI coordinate helpers.

use windows::Win32::Foundation::{HWND, LPARAM, POINT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DestroyWindow, IDC_ARROW, LoadCursorW,
    RegisterClassExW, RegisterWindowMessageW, WNDCLASSEXW, WNDPROC, WS_CLIPCHILDREN,
    WS_CLIPSIBLINGS, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::PCWSTR;

use crate::core::error::{AppError, AppResult};
use crate::platform::theme::{SystemTheme, apply_flyout_dwm};
use crate::platform::win_util::to_wide_null;
use crate::ui::layout::ScreenRect;

const WINDOW_CLASS: &str = "ToggleLangFlyoutWnd";

/// RAII wrapper guaranteeing an owned top-level `HWND` is destroyed even if initialization fails.
pub struct WindowGuard(HWND);

impl WindowGuard {
    pub fn create_flyout(theme: SystemTheme, wnd_proc: WNDPROC) -> AppResult<Self> {
        let class_name = to_wide_null(WINDOW_CLASS);
        let title = to_wide_null("ToggleLang");

        // SAFETY: Registering the flyout window class and creating our hidden topmost popup window.
        unsafe {
            let hinstance = GetModuleHandleW(None)
                .map_err(|e| AppError::Backend(format!("GetModuleHandleW failed: {e}")))?;

            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: wnd_proc,
                hInstance: hinstance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            RegisterClassExW(&wc);

            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                348,
                260,
                None,
                None,
                Some(hinstance.into()),
                None,
            )
            .map_err(|e| AppError::Backend(format!("CreateWindowExW failed: {e}")))?;

            apply_flyout_dwm(hwnd, theme.app_dark, theme.transparency_enabled);
            Ok(Self(hwnd))
        }
    }

    pub fn raw(&self) -> HWND {
        self.0
    }
}

impl Drop for WindowGuard {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            // SAFETY: Destroying the owned flyout window on cleanup or init error.
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
}

pub fn register_taskbar_created_message() -> u32 {
    let wide = to_wide_null("TaskbarCreated");
    // SAFETY: NUL-terminated UTF-16 message name.
    unsafe { RegisterWindowMessageW(PCWSTR(wide.as_ptr())) }
}

pub fn default_anchor_point() -> POINT {
    let mut pt = POINT::default();
    // SAFETY: Reading monitor work area near primary screen bottom-right as fallback anchor.
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            pt.x = info.rcWork.right - 24;
            pt.y = info.rcWork.bottom;
        }
    }
    pt
}

pub fn monitor_work_area_and_dpi(anchor: POINT) -> (ScreenRect, u32) {
    // SAFETY: Querying monitor work area and effective DPI for the monitor containing `anchor`.
    unsafe {
        let monitor = MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let work = if GetMonitorInfoW(monitor, &mut info).as_bool() {
            ScreenRect {
                left: info.rcWork.left,
                top: info.rcWork.top,
                right: info.rcWork.right,
                bottom: info.rcWork.bottom,
            }
        } else {
            ScreenRect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1040,
            }
        };

        let mut dpi_x = 96u32;
        let mut dpi_y = 96u32;
        let dpi = if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_ok() {
            dpi_x.max(96)
        } else {
            96
        };
        (work, dpi)
    }
}

pub fn dip_size_to_px(width_dip: f32, height_dip: f32, dpi: u32) -> (u32, u32) {
    let scale = dpi.max(96) as f32 / 96.0;
    (
        (width_dip * scale).round().max(1.0) as u32,
        (height_dip * scale).round().max(1.0) as u32,
    )
}

pub fn px_point_to_dip(x_px: i32, y_px: i32, dpi: u32) -> (f32, f32) {
    let scale = dpi.max(96) as f32 / 96.0;
    (x_px as f32 / scale, y_px as f32 / scale)
}

pub fn lparam_coords(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_dip_and_physical_pixels_at_high_dpi() {
        assert_eq!(dip_size_to_px(348.0, 260.0, 144), (522, 390));
        let (dx, dy) = px_point_to_dip(522, 390, 144);
        assert!((dx - 348.0).abs() < 1e-3 && (dy - 260.0).abs() < 1e-3);
    }

    #[test]
    fn unpacks_signed_negative_lparam_coordinates() {
        let packed = ((-12i16 as u16) as isize) | (((-34i16 as u16) as isize) << 16);
        assert_eq!(lparam_coords(LPARAM(packed)), (-12, -34));
    }
}
