use windows::Win32::Globalization::{
    GetLocaleInfoEx, LCIDToLocaleName, LOCALE_SISO639LANGNAME, LOCALE_SLOCALIZEDDISPLAYNAME,
    LOCALE_SNATIVELANGUAGENAME,
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, KEY_READ};
use windows::Win32::UI::Shell::SHLoadIndirectString;
use windows::core::PCWSTR;

use crate::platform::registry::{KEYBOARD_LAYOUTS_KEY, RegKey};
use crate::platform::win_util::{to_wide_null, wide_nul_to_string};

#[derive(Debug, Clone)]
pub struct LanguageNames {
    pub bcp47: String,
    pub display_name: String,
    pub short_badge: String,
}

pub fn resolve_language_names(langid: u16, bcp47_hint: Option<&str>) -> LanguageNames {
    let locale_name = bcp47_hint
        .map(ToOwned::to_owned)
        .filter(|s| !s.is_empty())
        .or_else(|| lcid_to_locale_name(u32::from(langid)))
        .unwrap_or_else(|| format!("0x{langid:04X}"));

    let display_name = get_locale_info(&locale_name, LOCALE_SLOCALIZEDDISPLAYNAME)
        .unwrap_or_else(|| locale_name.clone());
    let native_lang = get_locale_info(&locale_name, LOCALE_SNATIVELANGUAGENAME);
    let iso639 = get_locale_info(&locale_name, LOCALE_SISO639LANGNAME);

    LanguageNames {
        short_badge: compute_short_badge(native_lang.as_deref(), iso639.as_deref(), &locale_name),
        bcp47: locale_name,
        display_name,
    }
}

/// Resolves the localized keyboard layout subtitle (e.g. `"US"`, `"Russian"`, `"Ukrainian (Enhanced)"`).
pub fn resolve_keyboard_layout_name(klid_hex: &str) -> Option<String> {
    let subkey = format!("{KEYBOARD_LAYOUTS_KEY}\\{klid_hex}");
    let key = RegKey::open(HKEY_LOCAL_MACHINE, &subkey, KEY_READ).ok()?;

    if let Some(indirect) = key.query_sz("Layout Display Name")
        && let Some(resolved) = load_indirect_string(&indirect)
        && !resolved.is_empty()
    {
        return Some(resolved);
    }
    key.query_sz("Layout Text")
}

/// Computes the exact expected high 16-bit word of the runtime `HKL` for an 8-hex-digit KLID.
/// - If the KLID registry key defines `"Layout Id"` (e.g. `"00A8"` for Ukrainian Enhanced `00020422`),
///   Windows constructs the `HKL` high word as `0xF000 | layout_id` (`0xF0A8`).
/// - Otherwise, the high word is `klid & 0xFFFF` (e.g. `0x0409` for `00000409`).
pub fn resolve_expected_hkl_hi(klid_hex: &str) -> Option<u16> {
    let subkey = format!("{KEYBOARD_LAYOUTS_KEY}\\{klid_hex}");
    if let Ok(key) = RegKey::open(HKEY_LOCAL_MACHINE, &subkey, KEY_READ)
        && let Some(layout_id_str) = key.query_sz("Layout Id")
        && let Ok(layout_id) = u16::from_str_radix(layout_id_str.trim(), 16)
    {
        return Some(0xF000 | (layout_id & 0x0FFF));
    }
    let klid = u32::from_str_radix(klid_hex, 16).ok()?;
    Some((klid & 0xFFFF) as u16)
}

/// Computes the 1–3 character Windows 11 taskbar badge (`ENG`, `РУС`, `УКР`, `DEU`, `日`, `中`, `한`).
pub fn compute_short_badge(
    native_lang: Option<&str>,
    iso639: Option<&str>,
    fallback: &str,
) -> String {
    if let Some(trimmed) = native_lang.map(str::trim)
        && let Some(first) = trimmed.chars().next()
    {
        if is_wide_script_char(first) {
            return first.to_string();
        }
        let letters: String = trimmed
            .chars()
            .filter(|c| c.is_alphabetic())
            .take(3)
            .collect();
        if !letters.is_empty() {
            return letters.to_uppercase();
        }
    }
    if let Some(iso) = iso639 {
        let up = iso.trim().to_uppercase();
        if !up.is_empty() {
            return up.chars().take(3).collect();
        }
    }
    fallback.chars().take(3).collect::<String>().to_uppercase()
}

fn is_wide_script_char(c: char) -> bool {
    let u = c as u32;
    (0x3040..=0x30FF).contains(&u) // Hiragana / Katakana
        || (0x4E00..=0x9FFF).contains(&u) // CJK Unified Ideographs
        || (0xAC00..=0xD7AF).contains(&u) // Hangul Syllables
        || (0x3400..=0x4DBF).contains(&u) // CJK Ext A
}

fn lcid_to_locale_name(lcid: u32) -> Option<String> {
    let mut buf = [0u16; 85];
    // SAFETY: `buf` is sized to `LOCALE_NAME_MAX_LENGTH` (85 UTF-16 code units).
    let len = unsafe { LCIDToLocaleName(lcid, Some(&mut buf), 0) };
    (len > 1).then(|| wide_nul_to_string(&buf[..len as usize]))
}

fn get_locale_info(locale: &str, lctype: u32) -> Option<String> {
    let wide_locale = to_wide_null(locale);
    let mut buf = [0u16; 256];
    // SAFETY: `wide_locale` is NUL-terminated and `buf` is valid writable UTF-16 storage.
    let len = unsafe { GetLocaleInfoEx(PCWSTR(wide_locale.as_ptr()), lctype, Some(&mut buf)) };
    (len > 1).then(|| wide_nul_to_string(&buf[..len as usize]))
}

fn load_indirect_string(source: &str) -> Option<String> {
    let wide_src = to_wide_null(source);
    let mut out = [0u16; 256];
    // SAFETY: `wide_src` is NUL-terminated and `out` is a writable UTF-16 buffer.
    let hr = unsafe { SHLoadIndirectString(PCWSTR(wide_src.as_ptr()), &mut out, None) };
    hr.is_ok().then(|| wide_nul_to_string(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_badge_matches_windows_11_taskbar() {
        assert_eq!(
            compute_short_badge(Some("English"), Some("en"), "en-US"),
            "ENG"
        );
        assert_eq!(
            compute_short_badge(Some("русский"), Some("ru"), "ru"),
            "РУС"
        );
        assert_eq!(
            compute_short_badge(Some("українська"), Some("uk"), "uk"),
            "УКР"
        );
        assert_eq!(
            compute_short_badge(Some("Deutsch"), Some("de"), "de-DE"),
            "DEU"
        );
        assert_eq!(
            compute_short_badge(Some("日本語"), Some("ja"), "ja-JP"),
            "日"
        );
        assert_eq!(compute_short_badge(Some("中文"), Some("zh"), "zh-CN"), "中");
        assert_eq!(
            compute_short_badge(Some("한국어"), Some("ko"), "ko-KR"),
            "한"
        );
    }
}
