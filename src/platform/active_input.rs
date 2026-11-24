use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, PostMessageW,
    SetForegroundWindow, WM_INPUTLANGCHANGEREQUEST,
};
use windows::core::BOOL;

use crate::platform::win_util::wide_nul_to_string;

/// Reads the live active `HKL` of the current foreground window (handling both Win32 and UWP hosts).
pub fn foreground_hkl() -> Option<isize> {
    // SAFETY: `GetForegroundWindow` returns NULL when no foreground window exists.
    let fg = unsafe { GetForegroundWindow() };
    if fg.0.is_null() {
        return None;
    }

    let target = find_uwp_core_window(fg).unwrap_or(fg);
    // SAFETY: `target` is a valid HWND; passing None for process ID only retrieves the thread ID.
    let tid = unsafe { GetWindowThreadProcessId(target, None) };
    if tid == 0 {
        return None;
    }

    // SAFETY: `GetKeyboardLayout` returns the active HKL for the given thread.
    let hkl = unsafe { GetKeyboardLayout(tid) };
    (!hkl.0.is_null()).then_some(hkl.0 as isize)
}

/// Captures the foreground window if it is an external app window (not our own flyout or taskbar).
pub fn capture_external_foreground(own_hwnd: HWND) -> Option<HWND> {
    // SAFETY: Standard Win32 query.
    let fg = unsafe { GetForegroundWindow() };
    if fg.0.is_null() || fg == own_hwnd {
        return None;
    }
    let class_name = window_class_name(fg);
    if class_name == "Shell_TrayWnd" || class_name == "Shell_SecondaryTrayWnd" {
        return None;
    }
    Some(fg)
}

/// Restores focus to `prev_foreground` and requests an immediate keyboard layout switch.
pub fn notify_window_layout_change(prev_foreground: Option<HWND>, hkl: Option<isize>) {
    let Some(hwnd) = prev_foreground else {
        return;
    };
    if hwnd.0.is_null() {
        return;
    }
    // SAFETY: Best-effort focus restoration and async input language change request.
    unsafe {
        let _ = SetForegroundWindow(hwnd);
        if let Some(hkl_val) = hkl {
            let target = find_uwp_core_window(hwnd).unwrap_or(hwnd);
            let _ = PostMessageW(
                Some(target),
                WM_INPUTLANGCHANGEREQUEST,
                WPARAM(0),
                LPARAM(hkl_val),
            );
        }
    }
}

fn find_uwp_core_window(parent: HWND) -> Option<HWND> {
    if window_class_name(parent) != "ApplicationFrameWindow" {
        return None;
    }

    struct SearchState {
        found: HWND,
    }

    unsafe extern "system" fn enum_proc(child: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` points to `SearchState` on the caller's stack for the duration of `EnumChildWindows`.
        let state = unsafe { &mut *(lparam.0 as *mut SearchState) };
        if window_class_name(child) == "Windows.UI.Core.CoreWindow" {
            state.found = child;
            return BOOL(0);
        }
        BOOL(1)
    }

    let mut state = SearchState {
        found: HWND::default(),
    };
    // SAFETY: `state` lives until `EnumChildWindows` returns.
    unsafe {
        let _ = EnumChildWindows(
            Some(parent),
            Some(enum_proc),
            LPARAM((&raw mut state) as isize),
        );
    }
    (!state.found.0.is_null()).then_some(state.found)
}

fn window_class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    // SAFETY: `buf` is valid writable UTF-16 storage.
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    if len <= 0 {
        return String::new();
    }
    wide_nul_to_string(&buf[..len as usize])
}
