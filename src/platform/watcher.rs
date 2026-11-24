use std::time::{Duration, Instant};

use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME,
    RegNotifyChangeKeyValue,
};
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, WaitForMultipleObjects,
};
use windows::core::PCWSTR;

use crate::core::error::{AppError, AppResult};
use crate::platform::registry::{PRELOAD_KEY, RegKey, USER_PROFILE_KEY};
use crate::platform::win_util::HandleGuard;

const WATCH_DEBOUNCE: Duration = Duration::from_millis(350);
const WAIT_TIMEOUT_CODE: u32 = 0x0000_0102;

/// Reusable asynchronous watcher for `User Profile` and `Keyboard Layout\Preload` registry changes.
/// Integrates seamlessly into either a blocking CLI loop or a Win32 `MsgWaitForMultipleObjects` GUI loop.
pub struct RegistryWatcher {
    user_profile_key: RegKey,
    preload_key: RegKey,
    ev_profile: HandleGuard,
    ev_preload: HandleGuard,
}

impl RegistryWatcher {
    pub fn start() -> AppResult<Self> {
        let user_profile_key = RegKey::open(HKEY_CURRENT_USER, USER_PROFILE_KEY, KEY_NOTIFY)?;
        let preload_key = RegKey::open(HKEY_CURRENT_USER, PRELOAD_KEY, KEY_NOTIFY)?;

        // SAFETY: Creating unnamed manual-reset Win32 event objects; owned by `HandleGuard`.
        let ev_profile = HandleGuard(
            unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
                .map_err(|e| AppError::Backend(format!("CreateEventW(profile) failed: {e}")))?,
        );
        let ev_preload = HandleGuard(
            unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
                .map_err(|e| AppError::Backend(format!("CreateEventW(preload) failed: {e}")))?,
        );

        let watcher = Self {
            user_profile_key,
            preload_key,
            ev_profile,
            ev_preload,
        };
        watcher.arm_index(0)?;
        watcher.arm_index(1)?;
        Ok(watcher)
    }

    pub fn handles(&self) -> [HANDLE; 2] {
        [self.ev_profile.0, self.ev_preload.0]
    }

    pub fn reset_and_rearm(&self, index: usize) -> AppResult<()> {
        let ev = match index {
            0 => self.ev_profile.0,
            1 => self.ev_preload.0,
            _ => return Ok(()),
        };
        // SAFETY: `ev` is a valid owned manual-reset event handle.
        unsafe {
            let _ = ResetEvent(ev);
        }
        self.arm_index(index)
    }

    fn arm_index(&self, index: usize) -> AppResult<()> {
        let (key, subtree, ev) = match index {
            0 => (&self.user_profile_key, true, self.ev_profile.0),
            1 => (&self.preload_key, false, self.ev_preload.0),
            _ => return Ok(()),
        };
        let filter = REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET;
        // SAFETY: `key` and `ev` live as long as `self`.
        let err = unsafe { RegNotifyChangeKeyValue(key.raw(), subtree, filter, ev, true) };
        if err == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(AppError::Backend(format!(
                "RegNotifyChangeKeyValue failed: {err:?}"
            )))
        }
    }
}

/// Watches `User Profile` and `Keyboard Layout\Preload` asynchronously with 0% idle CPU.
pub fn watch_language_registry<F>(mut on_change: F) -> AppResult<()>
where
    F: FnMut(&str),
{
    let watcher = RegistryWatcher::start()?;
    let handles = watcher.handles();
    let mut last_trigger: Option<Instant> = None;

    loop {
        let wait_ms = match last_trigger {
            Some(t) => {
                let elapsed = t.elapsed();
                if elapsed >= WATCH_DEBOUNCE {
                    last_trigger = None;
                    on_change("User Profile / Preload updated");
                    INFINITE
                } else {
                    (WATCH_DEBOUNCE - elapsed).as_millis() as u32
                }
            }
            None => INFINITE,
        };

        // SAFETY: Both event handles in `handles` are owned by `watcher`.
        let res = unsafe { WaitForMultipleObjects(&handles, false, wait_ms) };
        match res.0 {
            0 | 1 => {
                watcher.reset_and_rearm(res.0 as usize)?;
                last_trigger = Some(Instant::now());
            }
            WAIT_TIMEOUT_CODE => {
                if last_trigger.take().is_some() {
                    on_change("User Profile / Preload updated");
                }
            }
            other => {
                return Err(AppError::Backend(format!(
                    "WaitForMultipleObjects returned unexpected status 0x{other:X}"
                )));
            }
        }
    }
}
