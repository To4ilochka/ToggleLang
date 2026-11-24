use windows::Win32::System::Registry::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::PCWSTR;

use crate::core::error::{AppError, AppResult};
use crate::platform::registry::RegKey;
use crate::platform::win_util::to_wide_null;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ToggleLang";

pub fn is_autostart_enabled() -> bool {
    let Ok(key) = RegKey::open(HKEY_CURRENT_USER, RUN_KEY, KEY_READ) else {
        return false;
    };
    key.query_sz(VALUE_NAME)
        .is_some_and(|v| !v.trim().is_empty())
}

pub fn set_autostart_enabled(enabled: bool) -> AppResult<()> {
    let key = RegKey::open(HKEY_CURRENT_USER, RUN_KEY, KEY_SET_VALUE)?;
    if enabled {
        let exe = std::env::current_exe()
            .map_err(|e| AppError::Backend(format!("current_exe failed: {e}")))?;
        let quoted = format!("\"{}\"", exe.display());
        key.set_sz(VALUE_NAME, &quoted)
    } else {
        key.delete_value(VALUE_NAME)
    }
}

/// Opens the native Windows 11 Language & Region settings page (`ms-settings:regionlanguage`).
pub fn open_language_settings() -> AppResult<()> {
    let verb = to_wide_null("open");
    let target = to_wide_null("ms-settings:regionlanguage");
    // SAFETY: Both pointers are valid NUL-terminated UTF-16 strings.
    let res = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(target.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if (res.0 as isize) <= 32 {
        return Err(AppError::Backend(format!(
            "ShellExecuteW(ms-settings:regionlanguage) failed with code {}",
            res.0 as isize
        )));
    }
    Ok(())
}
