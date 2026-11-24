use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayIconMode {
    /// Default minimalist Windows 11 Fluent vector toggle icon.
    #[default]
    VectorIcon,
    /// Dynamic 2-letter language code (`EN`, `RU`, `UK`, `日`) inside a rounded keycap.
    KeycapCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    KeyboardLayout,
    InputProcessor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutProfile {
    /// Canonical uppercase TIP string for `InstallLayoutOrTip`, e.g. `0409:00000409`,
    /// `0422:00020422`, or `0411:{CLSID}{GUID}`.
    pub tip_id: String,
    pub kind: ProfileKind,
    pub langid: u16,
    /// Optional CLSID string (present for TSF Input Processors / IMEs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clsid: Option<String>,
    /// Optional profile GUID string (present for TSF Input Processors / IMEs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guid_profile: Option<String>,
    /// BCP-47 language tag (e.g. `en-US`, `ru`, `uk`).
    pub bcp47: String,
    /// Short 1-3 character taskbar badge (e.g. `ENG`, `РУС`, `УКР`, `日`).
    pub short_badge: String,
    /// Localized language display name (e.g. `English (United States)`, `Russian`, `Ukrainian`).
    pub language_name: String,
    /// Localized keyboard layout or IME name (e.g. `US`, `Russian`, `Ukrainian (Enhanced)`).
    pub layout_name: String,
    /// Whether this profile is currently enabled in Alt+Shift / Win+Space.
    pub enabled: bool,
}

impl LayoutProfile {
    /// Extracts the 8-hex-digit KLID for keyboard layouts (returns `None` for IMEs).
    pub fn klid_hex(&self) -> Option<String> {
        if self.kind != ProfileKind::KeyboardLayout {
            return None;
        }
        parse_klid_from_tip(&self.tip_id)
    }

    /// Short 2-letter (or 1-char CJK) uppercase code for the keycap tray icon (`EN`, `RU`, `UK`, `日`).
    pub fn keycap_code(&self) -> String {
        if self.short_badge.chars().count() == 1 {
            return self.short_badge.clone();
        }
        let primary = self.bcp47.split(['-', '_']).next().unwrap_or("");
        let ascii_letters: String = primary
            .chars()
            .filter(char::is_ascii_alphabetic)
            .take(2)
            .collect();
        if !ascii_letters.is_empty() {
            return ascii_letters.to_ascii_uppercase();
        }
        self.short_badge
            .chars()
            .take(2)
            .collect::<String>()
            .to_uppercase()
    }

    /// Updates localized display metadata from a freshly queried live profile.
    pub fn refresh_metadata_from(&mut self, live: &Self) {
        self.bcp47 = live.bcp47.clone();
        self.short_badge = live.short_badge.clone();
        self.language_name = live.language_name.clone();
        self.layout_name = live.layout_name.clone();
        self.clsid = live.clsid.clone();
        self.guid_profile = live.guid_profile.clone();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppState {
    pub version: u32,
    /// Tray icon appearance preference (defaults to minimalist vector icon).
    #[serde(default)]
    pub tray_icon_mode: TrayIconMode,
    /// Initial ordered list of TIP IDs captured on first run (augmented when user installs new ones).
    pub original_snapshot: Vec<String>,
    /// Canonical ordered list of all tracked layouts/TIPs.
    pub profiles: Vec<LayoutProfile>,
}

impl AppState {
    pub fn enabled_count(&self) -> usize {
        self.profiles.iter().filter(|p| p.enabled).count()
    }

    pub fn has_disabled_profiles(&self) -> bool {
        self.profiles.iter().any(|p| !p.enabled)
    }

    pub fn enabled_tip_ids(&self) -> Vec<String> {
        self.profiles
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.tip_id.clone())
            .collect()
    }

    /// Returns the tray keycap code for `active_idx`, falling back to the first enabled layout.
    pub fn keycap_code_for(&self, active_idx: Option<usize>) -> String {
        active_idx
            .and_then(|i| self.profiles.get(i))
            .or_else(|| self.profiles.iter().find(|p| p.enabled))
            .map(LayoutProfile::keycap_code)
            .unwrap_or_else(|| "EN".into())
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            version: 1,
            tray_icon_mode: TrayIconMode::default(),
            original_snapshot: Vec::new(),
            profiles: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub added: Vec<String>,
    pub removed_externally: Vec<String>,
    pub reappeared_externally: Vec<String>,
}

/// Extracts an uppercase 8-hex-digit KLID from a `<LangID>:<KLID>` TIP string.
pub fn parse_klid_from_tip(tip_id: &str) -> Option<String> {
    let (_, rhs) = tip_id.split_once(':')?;
    let cleaned = rhs.trim().trim_start_matches("0x").trim_start_matches("0X");
    if cleaned.len() == 8 && cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(cleaned.to_ascii_uppercase())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_and_prefixed_klids() {
        assert_eq!(
            parse_klid_from_tip("0422:00020422").as_deref(),
            Some("00020422")
        );
        assert_eq!(
            parse_klid_from_tip("0x0409:0x00000409").as_deref(),
            Some("00000409")
        );
        assert_eq!(
            parse_klid_from_tip(
                "0411:{03B5835F-F03C-411B-9CE2-AA23E1171E36}{A76C93D9-5523-4E90-AAFA-4DB112F9AC76}"
            ),
            None
        );
    }

    #[test]
    fn computes_two_letter_and_cjk_keycap_codes() {
        let make = |bcp47: &str, badge: &str| LayoutProfile {
            tip_id: "0409:00000409".into(),
            kind: ProfileKind::KeyboardLayout,
            langid: 0x0409,
            clsid: None,
            guid_profile: None,
            bcp47: bcp47.into(),
            short_badge: badge.into(),
            language_name: String::new(),
            layout_name: String::new(),
            enabled: true,
        };
        assert_eq!(make("en-US", "ENG").keycap_code(), "EN");
        assert_eq!(make("ru", "РУС").keycap_code(), "RU");
        assert_eq!(make("uk", "УКР").keycap_code(), "UK");
        assert_eq!(make("de-DE", "DEU").keycap_code(), "DE");
        assert_eq!(make("ja-JP", "日").keycap_code(), "日");
    }

    #[test]
    fn app_state_defaults_tray_icon_mode_to_vector_when_missing() {
        let legacy_json = r#"{"version":1,"original_snapshot":[],"profiles":[]}"#;
        let state: AppState = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(state.tray_icon_mode, TrayIconMode::VectorIcon);
    }
}
