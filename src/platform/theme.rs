use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWM_SYSTEMBACKDROP_TYPE, DWMSBT_MAINWINDOW, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DWMWINDOWATTRIBUTE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::System::LibraryLoader::GetProcAddress;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, KEY_READ};
use windows::Win32::UI::Controls::MARGINS;
use windows::core::{BOOL, PCSTR};

use crate::platform::registry::RegKey;
use crate::platform::win_util::LibraryGuard;

const PERSONALIZE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const EXPLORER_ACCENT_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent";
const DWM_KEY: &str = r"Software\Microsoft\Windows\DWM";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemTheme {
    /// Whether app surfaces (flyout, menus) use Dark Mode (`AppsUseLightTheme == 0`).
    pub app_dark: bool,
    /// Whether the taskbar / system tray uses Dark Mode (`SystemUsesLightTheme == 0`).
    pub taskbar_dark: bool,
    /// Primary Fluent 2 accent color (already shade-selected for dark vs light mode).
    pub accent: Rgb,
    /// Hover Fluent 2 accent shade.
    pub accent_hover: Rgb,
    /// Whether Windows transparency effects are enabled (`EnableTransparency != 0`).
    pub transparency_enabled: bool,
}

impl SystemTheme {
    pub fn read() -> Self {
        let (app_dark, taskbar_dark, transparency_enabled) = read_personalize();
        let (accent, accent_hover) = read_fluent_accent(app_dark);
        Self {
            app_dark,
            taskbar_dark,
            accent,
            accent_hover,
            transparency_enabled,
        }
    }
}

/// Applies Windows 11 DWM rounded corners, dark/light attribute, and Acrylic (or Mica fallback) backdrop.
pub fn apply_flyout_dwm(hwnd: HWND, app_dark: bool, use_acrylic: bool) {
    let dark_val = BOOL::from(app_dark);
    let corner = DWMWCP_ROUND;
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };

    // SAFETY: `hwnd` is a valid owned window handle; `set_dwm_attr` passes exact ABI sizes.
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        let _ = set_dwm_attr(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark_val);
        let _ = set_dwm_attr(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner);

        let preferred: DWM_SYSTEMBACKDROP_TYPE = if use_acrylic {
            DWMSBT_TRANSIENTWINDOW
        } else {
            DWMSBT_MAINWINDOW
        };
        if set_dwm_attr(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &preferred).is_err() {
            let fallback = DWMSBT_MAINWINDOW;
            let _ = set_dwm_attr(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &fallback);
        }
    }
}

/// Enables native Windows 11 dark-mode styling on standard Win32 popup context menus
/// via `uxtheme.dll` ordinals 135 (`SetPreferredAppMode`) and 136 (`FlushMenuThemes`).
pub fn sync_win32_menu_theme(app_dark: bool) {
    #[repr(i32)]
    enum PreferredAppMode {
        AllowDark = 1,
        ForceDark = 2,
        ForceLight = 3,
    }
    type SetPreferredAppModeFn = unsafe extern "system" fn(PreferredAppMode) -> i32;
    type FlushMenuThemesFn = unsafe extern "system" fn();

    let Ok(library) = LibraryGuard::load_system32("uxtheme.dll") else {
        return;
    };
    let module = library.raw();

    // SAFETY: Calling well-known Win10/11 `uxtheme.dll` ordinals if exported; `library` frees on drop.
    unsafe {
        if let Some(proc) = GetProcAddress(module, PCSTR(135usize as *const u8)) {
            let set_mode: SetPreferredAppModeFn = std::mem::transmute_copy(&proc);
            let mode = if app_dark {
                PreferredAppMode::ForceDark
            } else {
                PreferredAppMode::ForceLight
            };
            let _ = set_mode(PreferredAppMode::AllowDark);
            let _ = set_mode(mode);
        }

        if let Some(proc) = GetProcAddress(module, PCSTR(136usize as *const u8)) {
            let flush: FlushMenuThemesFn = std::mem::transmute_copy(&proc);
            flush();
        }
    }
}

unsafe fn set_dwm_attr<T>(
    hwnd: HWND,
    attr: DWMWINDOWATTRIBUTE,
    value: &T,
) -> windows::core::Result<()> {
    // SAFETY: Caller guarantees `hwnd` is valid and `T` matches the ABI type of `attr`.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attr,
            (value as *const T).cast(),
            std::mem::size_of::<T>() as u32,
        )
    }
}

fn read_personalize() -> (bool, bool, bool) {
    let Ok(key) = RegKey::open(HKEY_CURRENT_USER, PERSONALIZE_KEY, KEY_READ) else {
        return (true, true, true);
    };
    let app_dark = key.query_dword("AppsUseLightTheme").unwrap_or(0) == 0;
    let taskbar_dark = key.query_dword("SystemUsesLightTheme").unwrap_or(0) == 0;
    let transparency = key.query_dword("EnableTransparency").unwrap_or(1) != 0;
    (app_dark, taskbar_dark, transparency)
}

fn read_fluent_accent(app_dark: bool) -> (Rgb, Rgb) {
    if let Ok(key) = RegKey::open(HKEY_CURRENT_USER, EXPLORER_ACCENT_KEY, KEY_READ)
        && let Some(bytes) = key.query_binary("AccentPalette")
        && let Some(pair) = parse_accent_palette(&bytes, app_dark)
    {
        return pair;
    }

    if let Ok(key) = RegKey::open(HKEY_CURRENT_USER, DWM_KEY, KEY_READ)
        && let Some(abgr) = key.query_dword("AccentColor")
    {
        let base = Rgb::new(
            (abgr & 0xFF) as u8,
            ((abgr >> 8) & 0xFF) as u8,
            ((abgr >> 16) & 0xFF) as u8,
        );
        return (base, base);
    }

    // Standard Windows 11 Fluent Blue fallback.
    if app_dark {
        (Rgb::new(0x60, 0xCD, 0xFF), Rgb::new(0x99, 0xEB, 0xFF))
    } else {
        (Rgb::new(0x00, 0x5F, 0xB8), Rgb::new(0x19, 0x75, 0xC5))
    }
}

/// Parses Windows 11's 8-entry RGBA `Explorer\Accent\AccentPalette` binary blob:
/// `[0]=Light3, [1]=Light2 (Dark primary), [2]=Light1 (Dark hover), [3]=Base (Light hover),
///  [4]=Dark1 (Light primary), [5]=Dark2, [6]=Dark3, [7]=Complementary`.
fn parse_accent_palette(bytes: &[u8], app_dark: bool) -> Option<(Rgb, Rgb)> {
    if bytes.len() < 28 {
        return None;
    }
    let shade = |idx: usize| Rgb::new(bytes[idx * 4], bytes[idx * 4 + 1], bytes[idx * 4 + 2]);
    Some(if app_dark {
        (shade(1), shade(2))
    } else {
        (shade(4), shade(3))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dark_and_light_accent_palette_shades() {
        let mut blob = [0u8; 32];
        // Shade 1 (Dark primary)
        blob[4..7].copy_from_slice(&[0x60, 0xCD, 0xFF]);
        // Shade 2 (Dark hover)
        blob[8..11].copy_from_slice(&[0x99, 0xEB, 0xFF]);
        // Shade 3 (Light hover)
        blob[12..15].copy_from_slice(&[0x19, 0x75, 0xC5]);
        // Shade 4 (Light primary)
        blob[16..19].copy_from_slice(&[0x00, 0x5F, 0xB8]);

        assert_eq!(
            parse_accent_palette(&blob, true),
            Some((Rgb::new(0x60, 0xCD, 0xFF), Rgb::new(0x99, 0xEB, 0xFF)))
        );
        assert_eq!(
            parse_accent_palette(&blob, false),
            Some((Rgb::new(0x00, 0x5F, 0xB8), Rgb::new(0x19, 0x75, 0xC5)))
        );
        assert_eq!(parse_accent_palette(&blob[..16], true), None);
    }
}
