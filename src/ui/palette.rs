use crate::platform::theme::{Rgb, SystemTheme};

/// RGBA color in premultiplied-ready normalized `[0.0, 1.0]` space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0.0);

    pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a,
        }
    }

    pub const fn from_rgb(rgb: Rgb, a: f32) -> Self {
        Self::rgba(rgb.r, rgb.g, rgb.b, a)
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        let k = t.clamp(0.0, 1.0);
        Self {
            r: self.r + (other.r - self.r) * k,
            g: self.g + (other.g - self.g) * k,
            b: self.b + (other.b - self.b) * k,
            a: self.a + (other.a - self.a) * k,
        }
    }

    pub fn scale_alpha(self, factor: f32) -> Self {
        Self {
            a: (self.a * factor.clamp(0.0, 1.0)).clamp(0.0, 1.0),
            ..self
        }
    }
}

/// Complete Windows 11 Fluent 2 design-token palette derived from `SystemTheme`.
#[derive(Debug, Clone, Copy)]
pub struct FluentPalette {
    /// Subtle surface tint rendered over DWM Acrylic (or opaque fallback when transparency is off).
    pub surface_tint: Color,
    pub footer_tint: Color,
    pub divider: Color,

    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_disabled: Color,
    pub text_secondary_disabled: Color,

    pub keycap_bg: Color,
    pub keycap_border: Color,
    pub keycap_text: Color,

    pub row_active_bg: Color,
    pub row_hover_bg: Color,
    pub row_active_hover_bg: Color,
    pub row_pressed_bg: Color,
    pub focus_ring: Color,

    pub accent: Color,
    pub accent_hover: Color,
    pub accent_locked: Color,

    pub toggle_off_bg: Color,
    pub toggle_off_hover_bg: Color,
    pub toggle_off_border: Color,
    pub toggle_off_knob: Color,
    pub toggle_on_knob: Color,
    pub toggle_locked_knob: Color,
}

impl FluentPalette {
    pub fn from_theme(theme: SystemTheme) -> Self {
        let accent = Color::from_rgb(theme.accent, 1.0);
        let accent_hover = Color::from_rgb(theme.accent_hover, 1.0);

        if theme.app_dark {
            // DWM Acrylic already supplies Windows 11's dark luminosity/tint; keep overlay subtle
            // when transparency is enabled and fall back to opaque surface when disabled.
            let surface_alpha = if theme.transparency_enabled {
                0.12
            } else {
                1.0
            };
            Self {
                surface_tint: Color::rgba(0x20, 0x20, 0x20, surface_alpha),
                footer_tint: Color::rgba(
                    0x12,
                    0x12,
                    0x12,
                    if theme.transparency_enabled {
                        0.22
                    } else {
                        1.0
                    },
                ),
                divider: Color::rgba(0xFF, 0xFF, 0xFF, 0.08),

                text_primary: Color::rgba(0xFF, 0xFF, 0xFF, 0.96),
                text_secondary: Color::rgba(0xFF, 0xFF, 0xFF, 0.66),
                text_disabled: Color::rgba(0xFF, 0xFF, 0xFF, 0.42),
                text_secondary_disabled: Color::rgba(0xFF, 0xFF, 0xFF, 0.30),

                keycap_bg: Color::rgba(0xFF, 0xFF, 0xFF, 0.045),
                keycap_border: Color::rgba(0xFF, 0xFF, 0xFF, 0.16),
                keycap_text: Color::rgba(0xFF, 0xFF, 0xFF, 0.72),

                row_active_bg: Color::rgba(0xFF, 0xFF, 0xFF, 0.062),
                row_hover_bg: Color::rgba(0xFF, 0xFF, 0xFF, 0.062),
                row_active_hover_bg: Color::rgba(0xFF, 0xFF, 0xFF, 0.095),
                row_pressed_bg: Color::rgba(0xFF, 0xFF, 0xFF, 0.042),
                focus_ring: Color::rgba(0xFF, 0xFF, 0xFF, 0.55),

                accent,
                accent_hover,
                accent_locked: Color::from_rgb(theme.accent, 0.42),

                toggle_off_bg: Color::rgba(0x00, 0x00, 0x00, 0.12),
                toggle_off_hover_bg: Color::rgba(0xFF, 0xFF, 0xFF, 0.07),
                toggle_off_border: Color::rgba(0xFF, 0xFF, 0xFF, 0.56),
                toggle_off_knob: Color::rgba(0xFF, 0xFF, 0xFF, 0.80),
                toggle_on_knob: Color::rgba(0x08, 0x08, 0x08, 0.94),
                toggle_locked_knob: Color::rgba(0x08, 0x08, 0x08, 0.55),
            }
        } else {
            let surface_alpha = if theme.transparency_enabled {
                0.16
            } else {
                1.0
            };
            Self {
                surface_tint: Color::rgba(0xF9, 0xF9, 0xF9, surface_alpha),
                footer_tint: Color::rgba(
                    0xE8,
                    0xE8,
                    0xE8,
                    if theme.transparency_enabled {
                        0.28
                    } else {
                        1.0
                    },
                ),
                divider: Color::rgba(0x00, 0x00, 0x00, 0.075),

                text_primary: Color::rgba(0x00, 0x00, 0x00, 0.895),
                text_secondary: Color::rgba(0x00, 0x00, 0x00, 0.61),
                text_disabled: Color::rgba(0x00, 0x00, 0x00, 0.40),
                text_secondary_disabled: Color::rgba(0x00, 0x00, 0x00, 0.28),

                keycap_bg: Color::rgba(0x00, 0x00, 0x00, 0.035),
                keycap_border: Color::rgba(0x00, 0x00, 0x00, 0.16),
                keycap_text: Color::rgba(0x00, 0x00, 0x00, 0.66),

                row_active_bg: Color::rgba(0x00, 0x00, 0x00, 0.045),
                row_hover_bg: Color::rgba(0x00, 0x00, 0x00, 0.042),
                row_active_hover_bg: Color::rgba(0x00, 0x00, 0x00, 0.068),
                row_pressed_bg: Color::rgba(0x00, 0x00, 0x00, 0.028),
                focus_ring: Color::rgba(0x00, 0x00, 0x00, 0.52),

                accent,
                accent_hover,
                accent_locked: Color::from_rgb(theme.accent, 0.40),

                toggle_off_bg: Color::rgba(0x00, 0x00, 0x00, 0.025),
                toggle_off_hover_bg: Color::rgba(0x00, 0x00, 0x00, 0.055),
                toggle_off_border: Color::rgba(0x00, 0x00, 0x00, 0.46),
                toggle_off_knob: Color::rgba(0x00, 0x00, 0x00, 0.62),
                toggle_on_knob: Color::rgba(0xFF, 0xFF, 0xFF, 0.98),
                toggle_locked_knob: Color::rgba(0xFF, 0xFF, 0xFF, 0.72),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_lerp_clamps_endpoints() {
        let a = Color::rgba(0, 0, 0, 0.0);
        let b = Color::rgba(255, 255, 255, 1.0);
        assert_eq!(a.lerp(b, -0.5), a);
        assert_eq!(a.lerp(b, 1.5), b);
        let mid = a.lerp(b, 0.5);
        assert!((mid.r - 0.5).abs() < 1e-4 && (mid.a - 0.5).abs() < 1e-4);
    }

    #[test]
    fn derives_opaque_fallback_when_transparency_is_disabled() {
        let opaque_dark = FluentPalette::from_theme(SystemTheme {
            app_dark: true,
            taskbar_dark: true,
            accent: Rgb::new(0x60, 0xCD, 0xFF),
            accent_hover: Rgb::new(0x99, 0xEB, 0xFF),
            transparency_enabled: false,
        });
        assert_eq!(opaque_dark.surface_tint.a, 1.0);
        assert_eq!(opaque_dark.footer_tint.a, 1.0);

        let acrylic_light = FluentPalette::from_theme(SystemTheme {
            app_dark: false,
            taskbar_dark: false,
            accent: Rgb::new(0x00, 0x5F, 0xB8),
            accent_hover: Rgb::new(0x19, 0x75, 0xC5),
            transparency_enabled: true,
        });
        assert!(acrylic_light.surface_tint.a < 0.5);
        assert!(acrylic_light.footer_tint.a < 0.5);
    }
}
