use std::ffi::CStr;

use windows::Win32::Foundation::{HMODULE, MAX_PATH};
use windows::Win32::System::LibraryLoader::GetProcAddress;
use windows::core::{GUID, HRESULT, PCSTR, PCWSTR};

use crate::core::error::{AppError, AppResult};
use crate::core::model::parse_klid_from_tip;
use crate::platform::win_util::{LibraryGuard, to_wide_null, wide_nul_to_string};

pub const LOTP_INPUTPROCESSOR: u32 = 1;
pub const LOTP_KEYBOARDLAYOUT: u32 = 2;

pub const LOT_DEFAULT: u32 = 0x0001;

pub const ILOT_CLEANINSTALL: u32 = 0x0000_0040;
pub const SDLOT_NOAPPLYTOCURRENTSESSION: u32 = 0x0000_0001;

#[repr(C)]
#[derive(Clone)]
pub struct LayoutOrTipProfile {
    pub dw_profile_type: u32,
    pub langid: u16,
    pub clsid: GUID,
    pub guid_profile: GUID,
    pub catid: GUID,
    pub dw_substitute_layout: u32,
    pub dw_flags: u32,
    pub sz_id: [u16; MAX_PATH as usize],
}

impl Default for LayoutOrTipProfile {
    fn default() -> Self {
        Self {
            dw_profile_type: 0,
            langid: 0,
            clsid: GUID::zeroed(),
            guid_profile: GUID::zeroed(),
            catid: GUID::zeroed(),
            dw_substitute_layout: 0,
            dw_flags: 0,
            sz_id: [0u16; MAX_PATH as usize],
        }
    }
}

#[derive(Debug, Clone)]
pub struct RawEnabledProfile {
    pub profile_type: u32,
    pub langid: u16,
    pub clsid: GUID,
    pub guid_profile: GUID,
    pub substitute_layout: u32,
    pub flags: u32,
    pub tip_id: String,
}

impl RawEnabledProfile {
    pub fn is_keyboard_layout(&self) -> bool {
        self.profile_type == LOTP_KEYBOARDLAYOUT
    }

    pub fn is_default(&self) -> bool {
        (self.flags & LOT_DEFAULT) != 0
    }

    pub fn klid_hex(&self) -> Option<String> {
        self.is_keyboard_layout()
            .then(|| parse_klid_from_tip(&self.tip_id))
            .flatten()
    }
}

type EnumEnabledLayoutOrTipFn = unsafe extern "system" fn(
    psz_user_reg: PCWSTR,
    psz_system_reg: PCWSTR,
    psz_software_reg: PCWSTR,
    p_layout_or_tip_profile: *mut LayoutOrTipProfile,
    u_buf_length: u32,
) -> u32;

type InstallLayoutOrTipFn = unsafe extern "system" fn(psz: PCWSTR, dw_flags: u32) -> i32;
type SetDefaultLayoutOrTipFn = unsafe extern "system" fn(psz: PCWSTR, dw_flags: u32) -> i32;
type QueryLayoutOrTipStringFn = unsafe extern "system" fn(psz: PCWSTR, dw_flags: u32) -> HRESULT;

/// Safe dynamic loader and adapter for documented `C:\Windows\System32\input.dll` exports.
pub struct InputDll {
    _library: LibraryGuard,
    enum_enabled: EnumEnabledLayoutOrTipFn,
    install_layout_or_tip: InstallLayoutOrTipFn,
    set_default_layout_or_tip: SetDefaultLayoutOrTipFn,
    query_layout_or_tip_string: QueryLayoutOrTipStringFn,
}

impl InputDll {
    pub fn load() -> AppResult<Self> {
        let library = LibraryGuard::load_system32("input.dll")?;
        let module = library.raw();

        // SAFETY: Each symbol signature matches the official Microsoft Learn ABI for `input.dll`.
        // If any lookup fails, `library` drops and frees `input.dll` automatically via RAII.
        let (
            enum_enabled,
            install_layout_or_tip,
            set_default_layout_or_tip,
            query_layout_or_tip_string,
        ) = unsafe {
            (
                resolve_proc::<EnumEnabledLayoutOrTipFn>(module, c"EnumEnabledLayoutOrTip")?,
                resolve_proc::<InstallLayoutOrTipFn>(module, c"InstallLayoutOrTip")?,
                resolve_proc::<SetDefaultLayoutOrTipFn>(module, c"SetDefaultLayoutOrTip")?,
                resolve_proc::<QueryLayoutOrTipStringFn>(module, c"QueryLayoutOrTipString")?,
            )
        };

        Ok(Self {
            _library: library,
            enum_enabled,
            install_layout_or_tip,
            set_default_layout_or_tip,
            query_layout_or_tip_string,
        })
    }

    pub fn enum_enabled(&self) -> AppResult<Vec<RawEnabledProfile>> {
        // SAFETY: Passing NULL output buffer queries the required element count.
        let count = unsafe {
            (self.enum_enabled)(
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null_mut(),
                0,
            )
        };
        if count == 0 {
            return Ok(Vec::new());
        }

        let mut buf = vec![LayoutOrTipProfile::default(); count as usize];
        // SAFETY: `buf` has length `count` matching `LAYOUTORTIPPROFILE` C layout (584 bytes).
        let written = unsafe {
            (self.enum_enabled)(
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
                buf.as_mut_ptr(),
                count,
            )
        };

        let result = buf
            .into_iter()
            .take(written as usize)
            .filter_map(|item| {
                let tip_id = wide_nul_to_string(&item.sz_id);
                (!tip_id.is_empty()).then_some(RawEnabledProfile {
                    profile_type: item.dw_profile_type,
                    langid: item.langid,
                    clsid: item.clsid,
                    guid_profile: item.guid_profile,
                    substitute_layout: item.dw_substitute_layout,
                    flags: item.dw_flags,
                    tip_id,
                })
            })
            .collect();

        Ok(result)
    }

    pub fn validate_tip_string(&self, tip_list: &str) -> AppResult<()> {
        let wide = to_wide_null(tip_list);
        // SAFETY: `wide` is NUL-terminated UTF-16.
        let hr = unsafe { (self.query_layout_or_tip_string)(PCWSTR(wide.as_ptr()), 0) };
        if hr.is_ok() {
            Ok(())
        } else {
            Err(AppError::Backend(format!(
                "QueryLayoutOrTipString rejected '{tip_list}' (HRESULT 0x{:08X})",
                hr.0 as u32
            )))
        }
    }

    /// Atomically replaces the enabled layout/TIP set with `tips` in the exact order given.
    pub fn apply_enabled_list(&self, tips: &[String]) -> AppResult<()> {
        if tips.is_empty() {
            return Err(AppError::EmptyLayoutList);
        }

        let joined = tips.join(";");
        self.validate_tip_string(&joined)?;

        let wide_joined = to_wide_null(&joined);
        // SAFETY: `wide_joined` is a validated NUL-terminated semicolon-delimited TIP string.
        let ok = unsafe {
            (self.install_layout_or_tip)(PCWSTR(wide_joined.as_ptr()), ILOT_CLEANINSTALL)
        };
        if ok == 0 {
            return Err(AppError::Backend(format!(
                "InstallLayoutOrTip(ILOT_CLEANINSTALL) returned FALSE for '{joined}'"
            )));
        }

        // Persist #1 active layout as default without forcibly overriding current typing layout.
        let wide_first = to_wide_null(&tips[0]);
        // SAFETY: `wide_first` is NUL-terminated UTF-16.
        let def_ok = unsafe {
            (self.set_default_layout_or_tip)(
                PCWSTR(wide_first.as_ptr()),
                SDLOT_NOAPPLYTOCURRENTSESSION,
            )
        };
        if def_ok == 0 {
            return Err(AppError::Backend(format!(
                "SetDefaultLayoutOrTip returned FALSE for '{}'",
                tips[0]
            )));
        }

        Ok(())
    }
}

unsafe fn resolve_proc<T>(module: HMODULE, name: &CStr) -> AppResult<T> {
    // SAFETY: Caller guarantees `module` is valid and `T` matches the exported ABI.
    let proc = unsafe { GetProcAddress(module, PCSTR(name.as_ptr().cast::<u8>())) }
        .ok_or_else(|| AppError::Backend(format!("Export {name:?} not found in input.dll")))?;
    Ok(unsafe { std::mem::transmute_copy(&proc) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_or_tip_profile_matches_win64_abi_size() {
        assert_eq!(std::mem::size_of::<LayoutOrTipProfile>(), 584);
    }
}
