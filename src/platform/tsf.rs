use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, S_FALSE, S_OK};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::{
    CLSID_TF_InputProcessorProfiles, GUID_TFCAT_TIP_KEYBOARD, ITfInputProcessorProfileMgr,
    ITfInputProcessorProfiles, TF_INPUTPROCESSORPROFILE, TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE,
    TF_IPPMF_FORSESSION, TF_PROFILETYPE_INPUTPROCESSOR, TF_PROFILETYPE_KEYBOARDLAYOUT,
};
use windows::core::{GUID, Interface};

use crate::core::error::{AppError, AppResult};
use crate::platform::input_api::LOTP_KEYBOARDLAYOUT;

/// RAII STA COM apartment guard. Must always be dropped AFTER all COM interface wrappers.
pub struct ComGuard {
    should_uninit: bool,
}

impl ComGuard {
    pub fn init_sta() -> AppResult<Self> {
        // SAFETY: Standard per-thread COM initialization.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr == S_OK || hr == S_FALSE {
            Ok(Self {
                should_uninit: true,
            })
        } else if hr == RPC_E_CHANGED_MODE {
            Ok(Self {
                should_uninit: false,
            })
        } else {
            Err(AppError::Backend(format!(
                "CoInitializeEx failed: 0x{:08X}",
                hr.0 as u32
            )))
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.should_uninit {
            // SAFETY: Paired 1-to-1 with `CoInitializeEx` returning `S_OK` or `S_FALSE`.
            unsafe { CoUninitialize() };
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActiveInputProfile {
    pub profile_type: u32,
    pub langid: u16,
    pub clsid: GUID,
    pub guid_profile: GUID,
    pub hkl: isize,
}

pub struct TsfManager {
    profiles: ITfInputProcessorProfiles,
    profile_mgr: ITfInputProcessorProfileMgr,
    // Invariant: MUST remain the last field so Rust drops COM interfaces BEFORE `CoUninitialize()`.
    _com: ComGuard,
}

impl TsfManager {
    pub fn new() -> AppResult<Self> {
        let com = ComGuard::init_sta()?;
        // SAFETY: COM apartment is initialized on the current thread.
        let profiles: ITfInputProcessorProfiles = unsafe {
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)
        }
        .map_err(|e| {
            AppError::Backend(format!(
                "CoCreateInstance(CLSID_TF_InputProcessorProfiles) failed: {e}"
            ))
        })?;

        let profile_mgr: ITfInputProcessorProfileMgr = profiles.cast().map_err(|e| {
            AppError::Backend(format!(
                "QueryInterface(ITfInputProcessorProfileMgr) failed: {e}"
            ))
        })?;

        Ok(Self {
            profiles,
            profile_mgr,
            _com: com,
        })
    }

    pub fn get_active_profile(&self) -> AppResult<ActiveInputProfile> {
        let mut profile = TF_INPUTPROCESSORPROFILE::default();
        // SAFETY: `profile` is valid writable memory; COM apartment is active.
        unsafe {
            self.profile_mgr
                .GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut profile)
        }
        .map_err(|e| AppError::Backend(format!("GetActiveProfile failed: {e}")))?;

        Ok(ActiveInputProfile {
            profile_type: profile.dwProfileType,
            langid: profile.langid,
            clsid: profile.clsid,
            guid_profile: profile.guidProfile,
            hkl: profile.hkl.0 as isize,
        })
    }

    pub fn enumerate_tsf_profiles(&self) -> AppResult<Vec<TF_INPUTPROCESSORPROFILE>> {
        // SAFETY: Enumerates all registered TSF keyboard/TIP profiles (`langid = 0`).
        let enumerator = unsafe { self.profile_mgr.EnumProfiles(0) }
            .map_err(|e| AppError::Backend(format!("EnumProfiles failed: {e}")))?;

        let mut out = Vec::new();
        loop {
            let mut batch = [TF_INPUTPROCESSORPROFILE::default(); 16];
            let mut fetched = 0u32;
            // SAFETY: `batch` is valid writable slice of 16 structs; `fetched <= 16`.
            let hr = unsafe { enumerator.Next(&mut batch, &mut fetched) };
            if fetched > 0 {
                out.extend_from_slice(&batch[..fetched as usize]);
            }
            if hr.is_err() || fetched == 0 {
                break;
            }
        }
        Ok(out)
    }

    pub fn activate_profile(
        &self,
        profile_type: u32,
        langid: u16,
        clsid: &GUID,
        guid_profile: &GUID,
        hkl: isize,
    ) -> AppResult<()> {
        let tsf_type = if profile_type == LOTP_KEYBOARDLAYOUT {
            TF_PROFILETYPE_KEYBOARDLAYOUT
        } else {
            TF_PROFILETYPE_INPUTPROCESSOR
        };

        let flags = TF_IPPMF_FORSESSION | TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE;
        // SAFETY: All pointers and handles are valid for the duration of the COM call.
        unsafe {
            self.profile_mgr.ActivateProfile(
                tsf_type,
                langid,
                clsid,
                guid_profile,
                HKL(hkl as *mut core::ffi::c_void),
                flags,
            )
        }
        .map_err(|e| AppError::Backend(format!("ActivateProfile failed: {e}")))
    }

    pub fn get_tip_description(
        &self,
        clsid: &GUID,
        langid: u16,
        guid_profile: &GUID,
    ) -> Option<String> {
        // SAFETY: `GetLanguageProfileDescription` returns an owned `BSTR` wrapped safely by `windows-core`.
        let bstr = unsafe {
            self.profiles
                .GetLanguageProfileDescription(clsid, langid, guid_profile)
        }
        .ok()?;
        let text = bstr.to_string();
        (!text.is_empty()).then_some(text)
    }
}
