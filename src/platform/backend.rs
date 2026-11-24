use windows::Win32::UI::TextServices::{
    TF_INPUTPROCESSORPROFILE, TF_PROFILETYPE_INPUTPROCESSOR, TF_PROFILETYPE_KEYBOARDLAYOUT,
};
use windows::core::GUID;

use crate::core::error::{AppError, AppResult};
use crate::core::model::{LayoutProfile, ProfileKind};
use crate::core::ports::InputBackend;
use crate::platform::input_api::{
    InputDll, LOTP_INPUTPROCESSOR, LOTP_KEYBOARDLAYOUT, RawEnabledProfile,
};
use crate::platform::locale::{
    resolve_expected_hkl_hi, resolve_keyboard_layout_name, resolve_language_names,
};
use crate::platform::registry::{UserProfileSnapshot, read_user_profile_snapshot};
use crate::platform::tsf::{ActiveInputProfile, TsfManager};
use crate::platform::win_util::{format_guid, parse_guid};

/// Concrete Windows 11 `InputBackend` adapter combining `input.dll`, TSF COM, Registry, and Locale APIs.
pub struct WindowsInputBackend {
    // Invariant: Drop TSF COM interfaces before unloading `input.dll`.
    tsf: TsfManager,
    input: InputDll,
}

impl WindowsInputBackend {
    pub fn new() -> AppResult<Self> {
        let input = InputDll::load()?;
        let tsf = TsfManager::new()?;
        Ok(Self { tsf, input })
    }

    pub fn raw_enabled(&self) -> AppResult<Vec<RawEnabledProfile>> {
        self.input.enum_enabled()
    }

    pub fn active_tsf_profile(&self) -> AppResult<ActiveInputProfile> {
        self.tsf.get_active_profile()
    }

    /// Resolves the live runtime `HKL` for a given keyboard layout profile (for `WM_INPUTLANGCHANGEREQUEST`).
    pub fn find_live_hkl(&self, profile: &LayoutProfile) -> Option<isize> {
        let tsf_profiles = self.tsf.enumerate_tsf_profiles().ok()?;
        let tsf_p = Self::find_matching_tsf_profile(profile, &tsf_profiles)?;
        let hkl = tsf_p.hkl.0 as isize;
        (hkl != 0).then_some(hkl)
    }

    /// Maps an arbitrary foreground window `HKL` to the matching enabled profile index.
    pub fn match_hkl_to_index(&self, hkl: isize, candidates: &[LayoutProfile]) -> Option<usize> {
        match_hkl_to_profile_index(hkl, candidates, resolve_expected_hkl_hi)
    }

    fn enrich_profile(
        &self,
        raw: &RawEnabledProfile,
        user_profile: &UserProfileSnapshot,
    ) -> LayoutProfile {
        let tip_upper = raw.tip_id.to_ascii_uppercase();
        let bcp47_hint = user_profile
            .tip_to_bcp47
            .get(&tip_upper)
            .map(String::as_str);
        let lang_names = resolve_language_names(raw.langid, bcp47_hint);

        let (kind, layout_name, clsid, guid_profile) = if raw.is_keyboard_layout() {
            let name = raw
                .klid_hex()
                .and_then(|klid| resolve_keyboard_layout_name(&klid))
                .unwrap_or_else(|| raw.tip_id.clone());
            (ProfileKind::KeyboardLayout, name, None, None)
        } else {
            let name = self
                .tsf
                .get_tip_description(&raw.clsid, raw.langid, &raw.guid_profile)
                .unwrap_or_else(|| "Input Method Editor".into());
            (
                ProfileKind::InputProcessor,
                name,
                Some(format_guid(&raw.clsid)),
                Some(format_guid(&raw.guid_profile)),
            )
        };

        LayoutProfile {
            tip_id: tip_upper,
            kind,
            langid: raw.langid,
            clsid,
            guid_profile,
            bcp47: lang_names.bcp47,
            short_badge: lang_names.short_badge,
            language_name: lang_names.display_name,
            layout_name,
            enabled: true,
        }
    }

    fn find_matching_tsf_profile(
        profile: &LayoutProfile,
        tsf_profiles: &[TF_INPUTPROCESSORPROFILE],
    ) -> Option<TF_INPUTPROCESSORPROFILE> {
        match profile.kind {
            ProfileKind::KeyboardLayout => {
                let expected_hi = profile
                    .klid_hex()
                    .and_then(|klid| resolve_expected_hkl_hi(&klid));

                if let Some(hi) = expected_hi
                    && let Some(found) = tsf_profiles.iter().find(|p| {
                        p.dwProfileType == TF_PROFILETYPE_KEYBOARDLAYOUT
                            && p.langid == profile.langid
                            && hkl_high_word(p.hkl.0 as isize) == hi
                    })
                {
                    return Some(*found);
                }

                tsf_profiles
                    .iter()
                    .find(|p| {
                        p.dwProfileType == TF_PROFILETYPE_KEYBOARDLAYOUT
                            && p.langid == profile.langid
                    })
                    .copied()
            }
            ProfileKind::InputProcessor => {
                let clsid = parsed_guid_or_zero(profile.clsid.as_deref());
                let guid = parsed_guid_or_zero(profile.guid_profile.as_deref());
                tsf_profiles
                    .iter()
                    .find(|p| {
                        p.dwProfileType == TF_PROFILETYPE_INPUTPROCESSOR
                            && p.langid == profile.langid
                            && p.clsid == clsid
                            && p.guidProfile == guid
                    })
                    .copied()
            }
        }
    }
}

impl InputBackend for WindowsInputBackend {
    fn list_enabled(&self) -> AppResult<Vec<LayoutProfile>> {
        let live_raw = self.input.enum_enabled()?;
        let user_profile = read_user_profile_snapshot().unwrap_or_default();
        Ok(live_raw
            .iter()
            .map(|raw| self.enrich_profile(raw, &user_profile))
            .collect())
    }

    fn apply_enabled_order(&self, tip_ids: &[String]) -> AppResult<()> {
        self.input.apply_enabled_list(tip_ids)
    }

    fn active_tip_id(&self, candidates: &[LayoutProfile]) -> Option<String> {
        let active = self.tsf.get_active_profile().ok()?;
        select_active_tip_id(&active, candidates, resolve_expected_hkl_hi)
    }

    fn activate_profile(&self, profile: &LayoutProfile) -> AppResult<()> {
        let tsf_profiles = self.tsf.enumerate_tsf_profiles()?;
        let tsf_p = Self::find_matching_tsf_profile(profile, &tsf_profiles).ok_or_else(|| {
            AppError::Backend(format!(
                "Could not find live TSF profile handle for '{}'",
                profile.tip_id
            ))
        })?;

        let ptype = if profile.kind == ProfileKind::KeyboardLayout {
            LOTP_KEYBOARDLAYOUT
        } else {
            LOTP_INPUTPROCESSOR
        };

        self.tsf.activate_profile(
            ptype,
            tsf_p.langid,
            &tsf_p.clsid,
            &tsf_p.guidProfile,
            tsf_p.hkl.0 as isize,
        )
    }
}

/// Pure HKL-to-profile index matching (shared by foreground polling & unit tests).
fn match_hkl_to_profile_index<F>(
    hkl: isize,
    candidates: &[LayoutProfile],
    resolve_hkl_hi: F,
) -> Option<usize>
where
    F: Fn(&str) -> Option<u16>,
{
    let langid = (hkl as usize & 0xFFFF) as u16;
    let active_hi = hkl_high_word(hkl);

    find_exact_keyboard_match(langid, active_hi, candidates, &resolve_hkl_hi).or_else(|| {
        // Fallback: first enabled profile with matching LANGID (covers IMEs and single-layout langs).
        candidates
            .iter()
            .position(|p| p.enabled && p.langid == langid)
    })
}

/// Pure TSF active-profile resolution using exact HKL/CLSID+GUID match with LANGID+type fallback.
fn select_active_tip_id<F>(
    active: &ActiveInputProfile,
    candidates: &[LayoutProfile],
    resolve_hkl_hi: F,
) -> Option<String>
where
    F: Fn(&str) -> Option<u16>,
{
    let exact_idx = match active.profile_type {
        TF_PROFILETYPE_KEYBOARDLAYOUT => find_exact_keyboard_match(
            active.langid,
            hkl_high_word(active.hkl),
            candidates,
            &resolve_hkl_hi,
        ),
        TF_PROFILETYPE_INPUTPROCESSOR => candidates.iter().position(|p| {
            p.enabled
                && p.langid == active.langid
                && p.kind == ProfileKind::InputProcessor
                && active.clsid == parsed_guid_or_zero(p.clsid.as_deref())
                && active.guid_profile == parsed_guid_or_zero(p.guid_profile.as_deref())
        }),
        _ => None,
    };

    if let Some(idx) = exact_idx {
        return Some(candidates[idx].tip_id.clone());
    }

    // Fallback pass: first enabled profile sharing the same language ID & type.
    candidates
        .iter()
        .find(|p| {
            p.enabled
                && p.langid == active.langid
                && matches!(
                    (p.kind, active.profile_type),
                    (ProfileKind::KeyboardLayout, TF_PROFILETYPE_KEYBOARDLAYOUT)
                        | (ProfileKind::InputProcessor, TF_PROFILETYPE_INPUTPROCESSOR)
                )
        })
        .map(|p| p.tip_id.clone())
}

fn find_exact_keyboard_match<F>(
    langid: u16,
    active_hi: u16,
    candidates: &[LayoutProfile],
    resolve_hkl_hi: &F,
) -> Option<usize>
where
    F: Fn(&str) -> Option<u16>,
{
    candidates.iter().position(|p| {
        p.enabled
            && p.langid == langid
            && p.kind == ProfileKind::KeyboardLayout
            && p.klid_hex()
                .and_then(|klid| resolve_hkl_hi(&klid))
                .is_some_and(|expected_hi| active_hi == expected_hi)
    })
}

fn hkl_high_word(hkl: isize) -> u16 {
    (((hkl as usize) >> 16) & 0xFFFF) as u16
}

fn parsed_guid_or_zero(raw: Option<&str>) -> GUID {
    raw.and_then(parse_guid).unwrap_or_else(GUID::zeroed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kbd(tip: &str, langid: u16, enabled: bool) -> LayoutProfile {
        LayoutProfile {
            tip_id: tip.into(),
            kind: ProfileKind::KeyboardLayout,
            langid,
            clsid: None,
            guid_profile: None,
            bcp47: "uk".into(),
            short_badge: "УКР".into(),
            language_name: "Ukrainian".into(),
            layout_name: "Layout".into(),
            enabled,
        }
    }

    #[test]
    fn distinguishes_multiple_layouts_under_same_langid_via_hkl_hi() {
        let candidates = vec![
            kbd("0422:00000422", 0x0422, true), // standard Ukrainian -> hi 0x0422
            kbd("0422:00020422", 0x0422, true), // Ukrainian Enhanced -> hi 0xF0A8
        ];
        let resolver = |klid: &str| match klid {
            "00000422" => Some(0x0422),
            "00020422" => Some(0xF0A8),
            _ => None,
        };

        let hkl_enhanced = ((0xF0A8usize << 16) | 0x0422) as isize;
        assert_eq!(
            match_hkl_to_profile_index(hkl_enhanced, &candidates, resolver),
            Some(1)
        );

        let active = ActiveInputProfile {
            profile_type: TF_PROFILETYPE_KEYBOARDLAYOUT,
            langid: 0x0422,
            clsid: GUID::zeroed(),
            guid_profile: GUID::zeroed(),
            hkl: hkl_enhanced,
        };
        assert_eq!(
            select_active_tip_id(&active, &candidates, resolver).as_deref(),
            Some("0422:00020422")
        );
    }

    #[test]
    fn skips_disabled_candidates_when_matching_hkl() {
        let candidates = vec![
            kbd("0422:00020422", 0x0422, false),
            kbd("0422:00000422", 0x0422, true),
        ];
        let resolver = |klid: &str| match klid {
            "00000422" => Some(0x0422),
            "00020422" => Some(0xF0A8),
            _ => None,
        };
        let hkl_enhanced = ((0xF0A8usize << 16) | 0x0422) as isize;
        // Falls back to the enabled Ukrainian profile rather than matching the disabled one.
        assert_eq!(
            match_hkl_to_profile_index(hkl_enhanced, &candidates, resolver),
            Some(1)
        );
    }
}
