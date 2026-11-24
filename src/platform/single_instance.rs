use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, PostMessageW, RegisterWindowMessageW,
};
use windows::core::PCWSTR;

use crate::core::error::{AppError, AppResult};
use crate::platform::win_util::{HandleGuard, to_wide_null};

const MUTEX_NAME: &str = r"Local\ToggleLang_SingleInstance_Mutex";
const WAKE_MESSAGE_NAME: &str = "ToggleLang_WakeFlyout_Msg";

pub struct SingleInstanceGuard {
    _handle: HandleGuard,
    wake_message: u32,
}

impl SingleInstanceGuard {
    /// Acquires the per-session single-instance mutex.
    /// Returns `Ok(Some(guard))` if this process is the primary instance, or `Ok(None)`
    /// after notifying the already-running instance to open its flyout.
    pub fn acquire_or_wake() -> AppResult<Option<Self>> {
        let wake_message = register_wake_message();
        let wide_name = to_wide_null(MUTEX_NAME);

        // SAFETY: `wide_name` is NUL-terminated UTF-16; returned handle is owned by `HandleGuard`.
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(wide_name.as_ptr())) }
            .map_err(|e| AppError::Backend(format!("CreateMutexW failed: {e}")))?;

        // SAFETY: Reading last Win32 error immediately after `CreateMutexW`.
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let guard = HandleGuard(handle);

        if already_exists {
            if wake_message != 0 {
                // SAFETY: Broadcasting a registered window message to wake the existing flyout.
                unsafe {
                    let _ = PostMessageW(Some(HWND_BROADCAST), wake_message, WPARAM(0), LPARAM(0));
                }
            }
            return Ok(None);
        }

        Ok(Some(Self {
            _handle: guard,
            wake_message,
        }))
    }

    pub fn wake_message(&self) -> u32 {
        self.wake_message
    }
}

fn register_wake_message() -> u32 {
    let wide = to_wide_null(WAKE_MESSAGE_NAME);
    // SAFETY: `wide` is NUL-terminated UTF-16.
    unsafe { RegisterWindowMessageW(PCWSTR(wide.as_ptr())) }
}
