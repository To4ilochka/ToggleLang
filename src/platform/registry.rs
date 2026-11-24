use std::collections::HashMap;

use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_BINARY, REG_DWORD, REG_MULTI_SZ, REG_SAM_FLAGS, REG_SZ,
    REG_VALUE_TYPE, RegCloseKey, RegDeleteValueW, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows::core::{PCWSTR, PWSTR};

use crate::core::error::{AppError, AppResult};
use crate::platform::win_util::{
    to_wide_null, utf16_bytes_to_string, utf16_bytes_to_u16s, wide_as_bytes,
};

pub const USER_PROFILE_KEY: &str = r"Control Panel\International\User Profile";
pub const KEYBOARD_LAYOUTS_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Keyboard Layouts";
pub const PRELOAD_KEY: &str = r"Keyboard Layout\Preload";
pub const SUBSTITUTES_KEY: &str = r"Keyboard Layout\Substitutes";
pub const CTF_SORTORDER_LANG_KEY: &str = r"Software\Microsoft\CTF\SortOrder\Language";

/// RAII wrapper for an open Windows Registry key (`HKEY`).
pub struct RegKey(HKEY);

impl RegKey {
    pub fn open(root: HKEY, subkey: &str, access: REG_SAM_FLAGS) -> AppResult<Self> {
        let wide = to_wide_null(subkey);
        let mut hkey = HKEY::default();
        // SAFETY: `wide` is NUL-terminated UTF-16; `hkey` is initialized by `RegOpenKeyExW` on success.
        let err = unsafe { RegOpenKeyExW(root, PCWSTR(wide.as_ptr()), 0, access, &mut hkey) };
        if err == ERROR_SUCCESS {
            Ok(Self(hkey))
        } else {
            Err(AppError::Backend(format!(
                "RegOpenKeyExW('{subkey}') failed: {err:?}"
            )))
        }
    }

    pub fn raw(&self) -> HKEY {
        self.0
    }

    pub fn query_sz(&self, value_name: &str) -> Option<String> {
        let bytes = self.query_raw_value(value_name, REG_SZ)?;
        Some(utf16_bytes_to_string(&bytes))
    }

    pub fn query_dword(&self, value_name: &str) -> Option<u32> {
        let bytes = self.query_raw_value(value_name, REG_DWORD)?;
        let arr: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
        Some(u32::from_le_bytes(arr))
    }

    pub fn query_binary(&self, value_name: &str) -> Option<Vec<u8>> {
        self.query_raw_value(value_name, REG_BINARY)
    }

    pub fn query_multi_sz(&self, value_name: &str) -> Option<Vec<String>> {
        let bytes = self.query_raw_value(value_name, REG_MULTI_SZ)?;
        let u16s = utf16_bytes_to_u16s(&bytes);
        Some(
            u16s.split(|&c| c == 0)
                .filter(|slice| !slice.is_empty())
                .map(String::from_utf16_lossy)
                .collect(),
        )
    }

    pub fn set_sz(&self, value_name: &str, value: &str) -> AppResult<()> {
        let wide_name = to_wide_null(value_name);
        let wide_val = to_wide_null(value);
        let bytes = wide_as_bytes(&wide_val);
        // SAFETY: `wide_name` and `wide_val` are NUL-terminated UTF-16 buffers valid for the call.
        let err =
            unsafe { RegSetValueExW(self.0, PCWSTR(wide_name.as_ptr()), 0, REG_SZ, Some(bytes)) };
        if err == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(AppError::Backend(format!(
                "RegSetValueExW('{value_name}') failed: {err:?}"
            )))
        }
    }

    pub fn delete_value(&self, value_name: &str) -> AppResult<()> {
        let wide_name = to_wide_null(value_name);
        // SAFETY: `wide_name` is NUL-terminated UTF-16.
        let err = unsafe { RegDeleteValueW(self.0, PCWSTR(wide_name.as_ptr())) };
        if err == ERROR_SUCCESS || err == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(AppError::Backend(format!(
                "RegDeleteValueW('{value_name}') failed: {err:?}"
            )))
        }
    }

    pub fn enum_subkeys(&self) -> Vec<String> {
        let mut result = Vec::new();
        let mut index = 0u32;
        loop {
            let mut name_buf = [0u16; 256];
            let mut name_len = name_buf.len() as u32;
            // SAFETY: `name_buf` has capacity `name_len` UTF-16 code units.
            let err = unsafe {
                RegEnumKeyExW(
                    self.0,
                    index,
                    PWSTR(name_buf.as_mut_ptr()),
                    &mut name_len,
                    None,
                    PWSTR::null(),
                    None,
                    None,
                )
            };
            match err {
                ERROR_SUCCESS => {
                    result.push(String::from_utf16_lossy(&name_buf[..name_len as usize]));
                }
                ERROR_MORE_DATA => {}
                ERROR_NO_MORE_ITEMS => break,
                _ => break,
            }
            index += 1;
        }
        result
    }

    pub fn enum_values(&self) -> Vec<(String, REG_VALUE_TYPE, Vec<u8>)> {
        let mut result = Vec::new();
        let mut index = 0u32;
        loop {
            let mut name_buf = [0u16; 256];
            let mut name_len = name_buf.len() as u32;
            let mut vtype = REG_VALUE_TYPE::default();
            let mut data_len = 0u32;

            // SAFETY: Probing value type and byte length with a valid name buffer.
            let err = unsafe {
                RegEnumValueW(
                    self.0,
                    index,
                    PWSTR(name_buf.as_mut_ptr()),
                    &mut name_len,
                    None,
                    Some(&mut vtype.0),
                    None,
                    Some(&mut data_len),
                )
            };
            match err {
                ERROR_SUCCESS => {
                    let mut data = vec![0u8; data_len as usize];
                    name_len = name_buf.len() as u32;
                    // SAFETY: `data` has capacity `data_len` bytes and `name_buf` has `name_len` code units.
                    let err2 = unsafe {
                        RegEnumValueW(
                            self.0,
                            index,
                            PWSTR(name_buf.as_mut_ptr()),
                            &mut name_len,
                            None,
                            Some(&mut vtype.0),
                            Some(data.as_mut_ptr()),
                            Some(&mut data_len),
                        )
                    };
                    if err2 == ERROR_SUCCESS {
                        data.truncate(data_len as usize);
                        let name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
                        result.push((name, vtype, data));
                    }
                }
                ERROR_MORE_DATA => {}
                ERROR_NO_MORE_ITEMS => break,
                _ => break,
            }
            index += 1;
        }
        result
    }

    /// Single DRY two-phase size-probe + buffer-read helper for typed registry values.
    fn query_raw_value(&self, value_name: &str, expected_type: REG_VALUE_TYPE) -> Option<Vec<u8>> {
        let wide_name = to_wide_null(value_name);
        let mut vtype = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        // SAFETY: `wide_name` is NUL-terminated; first call only probes byte length.
        let err = unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(wide_name.as_ptr()),
                None,
                Some(&mut vtype),
                None,
                Some(&mut size),
            )
        };
        if err != ERROR_SUCCESS || size == 0 || vtype != expected_type {
            return None;
        }

        let mut buf = vec![0u8; size as usize];
        // SAFETY: `buf` is allocated to `size` bytes as reported by the probe call.
        let err = unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(wide_name.as_ptr()),
                None,
                Some(&mut vtype),
                Some(buf.as_mut_ptr()),
                Some(&mut size),
            )
        };
        if err == ERROR_SUCCESS && vtype == expected_type {
            buf.truncate(size as usize);
            Some(buf)
        } else {
            None
        }
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: `self.0` was opened via `RegOpenKeyExW` and is owned exclusively by `self`.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UserProfileSnapshot {
    pub languages: Vec<String>,
    pub tip_to_bcp47: HashMap<String, String>,
    pub bcp47_tips: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct RegistryDiagnostics {
    pub user_profile: UserProfileSnapshot,
    pub preload: Vec<(String, String)>,
    pub substitutes: Vec<(String, String)>,
    pub ctf_sort_order: Vec<(String, String)>,
}

pub fn read_user_profile_snapshot() -> AppResult<UserProfileSnapshot> {
    let root = RegKey::open(HKEY_CURRENT_USER, USER_PROFILE_KEY, KEY_READ)?;
    let languages = root.query_multi_sz("Languages").unwrap_or_default();

    let mut tip_to_bcp47 = HashMap::new();
    let mut bcp47_tips = HashMap::new();

    for subkey_name in root.enum_subkeys() {
        let full_sub = format!("{USER_PROFILE_KEY}\\{subkey_name}");
        let Ok(sub) = RegKey::open(HKEY_CURRENT_USER, &full_sub, KEY_READ) else {
            continue;
        };
        let mut tips = Vec::new();
        for (val_name, val_type, _) in sub.enum_values() {
            if val_type == REG_DWORD && val_name.contains(':') {
                let normalized = val_name.to_ascii_uppercase();
                tip_to_bcp47.insert(normalized.clone(), subkey_name.clone());
                tips.push(normalized);
            }
        }
        bcp47_tips.insert(subkey_name, tips);
    }

    Ok(UserProfileSnapshot {
        languages,
        tip_to_bcp47,
        bcp47_tips,
    })
}

pub fn read_string_values(root: HKEY, subkey: &str) -> Vec<(String, String)> {
    let Ok(key) = RegKey::open(root, subkey, KEY_READ) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = key
        .enum_values()
        .into_iter()
        .filter_map(|(name, vtype, bytes)| {
            (vtype == REG_SZ).then(|| (name, utf16_bytes_to_string(&bytes)))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn read_diagnostics() -> AppResult<RegistryDiagnostics> {
    Ok(RegistryDiagnostics {
        user_profile: read_user_profile_snapshot()?,
        preload: read_string_values(HKEY_CURRENT_USER, PRELOAD_KEY),
        substitutes: read_string_values(HKEY_CURRENT_USER, SUBSTITUTES_KEY),
        ctf_sort_order: read_string_values(HKEY_CURRENT_USER, CTF_SORTORDER_LANG_KEY),
    })
}
