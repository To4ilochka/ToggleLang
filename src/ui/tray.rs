use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyIcon, DestroyMenu, HICON, MF_CHECKED, MF_GRAYED,
    MF_SEPARATOR, MF_STRING, MF_UNCHECKED, PostMessageW, SetForegroundWindow, SetMenuDefaultItem,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_NULL,
};
use windows::core::PCWSTR;

use crate::core::error::AppResult;
use crate::core::model::TrayIconMode;
use crate::platform::theme::sync_win32_menu_theme;
use crate::platform::win_util::to_wide_null;
use crate::ui::graphics::GraphicsFactories;
use crate::ui::tray_icon::TrayIconRenderer;

pub const WM_TRAY_CALLBACK: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;
const TRAY_ICON_ID: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum TrayMenuAction {
    OpenFlyout = 1001,
    ToggleTrayIconMode = 1002,
    ToggleAutostart = 1003,
    OpenSettings = 1004,
    RestoreAll = 1005,
    RestoreAndExit = 1006,
    Exit = 1007,
}

impl TrayMenuAction {
    const ALL: [Self; 7] = [
        Self::OpenFlyout,
        Self::ToggleTrayIconMode,
        Self::ToggleAutostart,
        Self::OpenSettings,
        Self::RestoreAll,
        Self::RestoreAndExit,
        Self::Exit,
    ];

    fn from_id(id: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|&action| action as u32 == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayMenuEntry {
    Item {
        action: TrayMenuAction,
        label: &'static str,
        checked: bool,
        enabled: bool,
        is_default: bool,
    },
    Separator,
}

fn build_menu_entries(
    icon_mode: TrayIconMode,
    autostart_enabled: bool,
    has_disabled_layouts: bool,
) -> [TrayMenuEntry; 10] {
    let exit_label = if has_disabled_layouts {
        "Exit without restoring"
    } else {
        "Exit"
    };

    [
        TrayMenuEntry::Item {
            action: TrayMenuAction::OpenFlyout,
            label: "Open keyboard layouts",
            checked: false,
            enabled: true,
            is_default: true,
        },
        TrayMenuEntry::Separator,
        TrayMenuEntry::Item {
            action: TrayMenuAction::ToggleTrayIconMode,
            label: "Show language code as tray icon",
            checked: icon_mode == TrayIconMode::KeycapCode,
            enabled: true,
            is_default: false,
        },
        TrayMenuEntry::Item {
            action: TrayMenuAction::ToggleAutostart,
            label: "Start with Windows",
            checked: autostart_enabled,
            enabled: true,
            is_default: false,
        },
        TrayMenuEntry::Separator,
        TrayMenuEntry::Item {
            action: TrayMenuAction::OpenSettings,
            label: "Windows language settings",
            checked: false,
            enabled: true,
            is_default: false,
        },
        TrayMenuEntry::Item {
            action: TrayMenuAction::RestoreAll,
            label: "Restore all layouts",
            checked: false,
            enabled: has_disabled_layouts,
            is_default: false,
        },
        TrayMenuEntry::Separator,
        TrayMenuEntry::Item {
            action: TrayMenuAction::RestoreAndExit,
            label: "Restore all layouts and exit",
            checked: false,
            enabled: has_disabled_layouts,
            is_default: false,
        },
        TrayMenuEntry::Item {
            action: TrayMenuAction::Exit,
            label: exit_label,
            checked: false,
            enabled: true,
            is_default: false,
        },
    ]
}

fn format_tray_tooltip(code: &str) -> String {
    format!("ToggleLang — {code}\nLeft-click: switch or toggle layouts\nRight-click: options")
}

/// Manages the system tray icon lifecycle (`Shell_NotifyIconW`), `TaskbarCreated` recovery,
/// and native dark/light Win32 context menu. Delegates pixel synthesis to `TrayIconRenderer`.
pub struct TrayController {
    hwnd: HWND,
    icon_renderer: TrayIconRenderer,
    current_icon: Option<HICON>,
    current_mode: TrayIconMode,
    current_code: String,
    current_taskbar_dark: bool,
    current_dpi: u32,
    added: bool,
}

impl TrayController {
    pub fn new(
        hwnd: HWND,
        factories: GraphicsFactories,
        mode: TrayIconMode,
        initial_code: &str,
        taskbar_dark: bool,
        dpi: u32,
    ) -> AppResult<Self> {
        let mut tray = Self {
            hwnd,
            icon_renderer: TrayIconRenderer::new(factories),
            current_icon: None,
            current_mode: mode,
            current_code: String::new(),
            current_taskbar_dark: taskbar_dark,
            current_dpi: dpi,
            added: false,
        };
        tray.add_or_recreate(mode, initial_code, taskbar_dark, dpi)?;
        Ok(tray)
    }

    /// Registers (or re-registers after `explorer.exe` restart) the tray icon.
    pub fn add_or_recreate(
        &mut self,
        mode: TrayIconMode,
        keycap_code: &str,
        taskbar_dark: bool,
        dpi: u32,
    ) -> AppResult<()> {
        let icon = self
            .icon_renderer
            .render_hicon(mode, keycap_code, taskbar_dark, dpi)?;
        self.replace_icon_handle(icon);
        self.current_mode = mode;
        self.current_code = keycap_code.to_string();
        self.current_taskbar_dark = taskbar_dark;
        self.current_dpi = dpi;

        let mut nid = self.base_nid(keycap_code);
        // SAFETY: `nid` is properly sized with valid `hwnd` and `hIcon`.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
        }
        self.added = true;
        Ok(())
    }

    /// Updates the tray icon & tooltip only when appearance or active language actually changed.
    pub fn update_icon(
        &mut self,
        mode: TrayIconMode,
        keycap_code: &str,
        taskbar_dark: bool,
        dpi: u32,
    ) -> AppResult<()> {
        let visual_unchanged = self.added
            && self.current_mode == mode
            && self.current_taskbar_dark == taskbar_dark
            && self.current_dpi == dpi
            && (mode == TrayIconMode::VectorIcon || self.current_code == keycap_code);

        if visual_unchanged && self.current_code == keycap_code {
            return Ok(());
        }

        if !visual_unchanged {
            let icon = self
                .icon_renderer
                .render_hicon(mode, keycap_code, taskbar_dark, dpi)?;
            self.replace_icon_handle(icon);
            self.current_mode = mode;
            self.current_taskbar_dark = taskbar_dark;
            self.current_dpi = dpi;
        }
        self.current_code = keycap_code.to_string();

        let nid = self.base_nid(keycap_code);
        // SAFETY: `nid` references valid owned `hIcon` and `hwnd`.
        let ok = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
        if !ok.as_bool() {
            self.add_or_recreate(mode, keycap_code, taskbar_dark, dpi)?;
        }
        Ok(())
    }

    /// Displays the native Windows 11 dark/light-aware context menu at `pt`.
    pub fn show_context_menu(
        &self,
        pt: POINT,
        app_dark: bool,
        icon_mode: TrayIconMode,
        autostart_enabled: bool,
        has_disabled_layouts: bool,
    ) -> Option<TrayMenuAction> {
        sync_win32_menu_theme(app_dark);

        // SAFETY: Creating, populating, tracking, and destroying a transient popup menu.
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return None;
            };

            for entry in build_menu_entries(icon_mode, autostart_enabled, has_disabled_layouts) {
                match entry {
                    TrayMenuEntry::Separator => {
                        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                    }
                    TrayMenuEntry::Item {
                        action,
                        label,
                        checked,
                        enabled,
                        is_default,
                    } => {
                        let mut flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
                        if !enabled {
                            flags |= MF_GRAYED;
                        }
                        let wide = to_wide_null(label);
                        let _ = AppendMenuW(menu, flags, action as usize, PCWSTR(wide.as_ptr()));
                        if is_default {
                            let _ = SetMenuDefaultItem(menu, action as u32, 0);
                        }
                    }
                }
            }

            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenuEx(
                menu,
                (TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RETURNCMD).0,
                pt.x,
                pt.y,
                self.hwnd,
                None,
            );
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);

            TrayMenuAction::from_id(cmd.0 as u32)
        }
    }

    fn base_nid(&self, code: &str) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ICON_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: WM_TRAY_CALLBACK,
            hIcon: self.current_icon.unwrap_or_default(),
            ..Default::default()
        };

        let tooltip = format_tray_tooltip(code);
        let wide_tip: Vec<u16> = tooltip.encode_utf16().collect();
        let max_len = nid.szTip.len().saturating_sub(1);
        let copy_len = wide_tip.len().min(max_len);
        nid.szTip[..copy_len].copy_from_slice(&wide_tip[..copy_len]);
        nid
    }

    fn replace_icon_handle(&mut self, new_icon: HICON) {
        if let Some(old) = self.current_icon.replace(new_icon) {
            // SAFETY: `old` is an owned `HICON` created via `CreateIconIndirect`.
            unsafe {
                let _ = DestroyIcon(old);
            }
        }
    }
}

impl Drop for TrayController {
    fn drop(&mut self) {
        if self.added {
            let nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: TRAY_ICON_ID,
                ..Default::default()
            };
            // SAFETY: Removing our registered notification icon on clean shutdown.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
        }
        if let Some(icon) = self.current_icon.take() {
            // SAFETY: `icon` is an owned `HICON`.
            unsafe {
                let _ = DestroyIcon(icon);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_action_ids_round_trip() {
        for action in TrayMenuAction::ALL {
            assert_eq!(TrayMenuAction::from_id(action as u32), Some(action));
        }
        assert_eq!(TrayMenuAction::from_id(0), None);
        assert_eq!(TrayMenuAction::from_id(9999), None);
    }

    #[test]
    fn menu_entries_reflect_disabled_state_and_english_labels() {
        let when_all_enabled = build_menu_entries(TrayIconMode::KeycapCode, false, false);
        assert!(when_all_enabled.iter().any(|e| matches!(
            e,
            TrayMenuEntry::Item {
                action: TrayMenuAction::OpenFlyout,
                label: "Open keyboard layouts",
                is_default: true,
                enabled: true,
                ..
            }
        )));
        assert!(when_all_enabled.iter().any(|e| matches!(
            e,
            TrayMenuEntry::Item {
                action: TrayMenuAction::ToggleTrayIconMode,
                label: "Show language code as tray icon",
                checked: true,
                ..
            }
        )));
        assert!(when_all_enabled.iter().any(|e| matches!(
            e,
            TrayMenuEntry::Item {
                action: TrayMenuAction::RestoreAll,
                label: "Restore all layouts",
                enabled: false,
                ..
            }
        )));
        assert!(when_all_enabled.iter().any(|e| matches!(
            e,
            TrayMenuEntry::Item {
                action: TrayMenuAction::RestoreAndExit,
                label: "Restore all layouts and exit",
                enabled: false,
                ..
            }
        )));
        assert!(when_all_enabled.iter().any(|e| matches!(
            e,
            TrayMenuEntry::Item {
                action: TrayMenuAction::Exit,
                label: "Exit",
                enabled: true,
                ..
            }
        )));

        let when_some_disabled = build_menu_entries(TrayIconMode::VectorIcon, true, true);
        assert!(when_some_disabled.iter().any(|e| matches!(
            e,
            TrayMenuEntry::Item {
                action: TrayMenuAction::ToggleAutostart,
                checked: true,
                ..
            }
        )));
        assert!(when_some_disabled.iter().any(|e| matches!(
            e,
            TrayMenuEntry::Item {
                action: TrayMenuAction::RestoreAll,
                enabled: true,
                ..
            }
        )));
        assert!(when_some_disabled.iter().any(|e| matches!(
            e,
            TrayMenuEntry::Item {
                action: TrayMenuAction::RestoreAndExit,
                enabled: true,
                ..
            }
        )));
        assert!(when_some_disabled.iter().any(|e| matches!(
            e,
            TrayMenuEntry::Item {
                action: TrayMenuAction::Exit,
                label: "Exit without restoring",
                enabled: true,
                ..
            }
        )));
    }

    #[test]
    fn tray_tooltip_fits_win32_buffer() {
        let tip = format_tray_tooltip("ENG");
        assert!(tip.encode_utf16().count() < 128);
        assert!(tip.contains("Left-click:"));
        assert!(tip.contains("Right-click:"));
    }
}
