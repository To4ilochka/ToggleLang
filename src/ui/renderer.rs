use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_POINT_2F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
    ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_FONT_WEIGHT_REGULAR, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
    IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{BitBlt, HDC, SRCCOPY};
use windows::core::PCWSTR;

use crate::core::error::{AppError, AppResult};
use crate::platform::win_util::{ScreenDcGuard, to_wide_null};
use crate::ui::animation::{ease_out_cubic, ease_out_quart};
use crate::ui::graphics::{DibSurface, GraphicsFactories, to_d2d_color, to_d2d_rect};
use crate::ui::layout::{FlyoutGeometry, HitTarget, ROW_RADIUS, RectF, RowGeometry};
use crate::ui::palette::{Color, FluentPalette};

pub struct RowRenderItem<'a> {
    pub badge: &'a str,
    pub language_name: &'a str,
    pub layout_name: &'a str,
    pub is_active: bool,
    pub is_enabled: bool,
    pub is_locked_last: bool,
    /// Smooth toggle animation progress: `0.0` (OFF) .. `1.0` (ON).
    pub toggle_t: f32,
    /// Smooth row card hover fade: `0.0` (idle) .. `1.0` (hovered).
    pub row_hover_t: f32,
    /// Smooth toggle knob hover scale: `0.0` (resting) .. `1.0` (hovered).
    pub knob_hover_t: f32,
    /// Smooth toggle knob press elongation: `0.0` (released) .. `1.0` (pressed).
    pub knob_press_t: f32,
}

pub struct FlyoutRenderView<'a> {
    pub geometry: &'a FlyoutGeometry,
    pub palette: &'a FluentPalette,
    pub rows: &'a [RowRenderItem<'a>],
    pub pressed: HitTarget,
    pub settings_hover_t: f32,
    pub restore_hover_t: f32,
    pub focused_row: Option<usize>,
    pub status_notice: Option<&'a str>,
    /// Open animation progress `0.0..=1.0` (drives quartic slide-up + content fade-in).
    pub open_progress: f32,
}

struct TextFormats {
    header_title: IDWriteTextFormat,
    keycap: IDWriteTextFormat,
    badge: IDWriteTextFormat,
    row_title: IDWriteTextFormat,
    row_subtitle: IDWriteTextFormat,
    footer_link: IDWriteTextFormat,
    footer_action: IDWriteTextFormat,
}

struct FooterButtonSpec<'a> {
    rect: RectF,
    label: &'a str,
    format: &'a IDWriteTextFormat,
    text_color: Color,
    text_inset_x: f32,
    hover_t: f32,
    is_pressed: bool,
}

/// Ultra-lightweight Direct2D (Software `ID2D1DCRenderTarget`) + DirectWrite Fluent 2 renderer.
/// Preserves full 32-bit premultiplied alpha over Windows 11's GPU-composited DWM Acrylic backdrop
/// while keeping frame render time under ~1 ms for silky VSync animations at 60–240 Hz.
pub struct FlyoutRenderer {
    hwnd: HWND,
    _d2d_factory: ID2D1Factory,
    target: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    formats: TextFormats,
    surface: DibSurface,
    dpi: f32,
}

impl FlyoutRenderer {
    pub fn new(
        hwnd: HWND,
        factories: &GraphicsFactories,
        width_px: u32,
        height_px: u32,
        dpi: u32,
    ) -> AppResult<Self> {
        let dpi_f = dpi.max(96) as f32;

        // SAFETY: Creating a software DC render target, reusable brush, and DirectWrite text formats.
        unsafe {
            let rt_props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: dpi_f,
                dpiY: dpi_f,
                ..Default::default()
            };

            let target = factories
                .d2d
                .CreateDCRenderTarget(&rt_props)
                .map_err(|e| AppError::Backend(format!("CreateDCRenderTarget failed: {e}")))?;
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

            let brush = target
                .CreateSolidColorBrush(&to_d2d_color(Color::TRANSPARENT), None)
                .map_err(|e| AppError::Backend(format!("CreateSolidColorBrush failed: {e}")))?;

            let formats = create_text_formats(&factories.dwrite)?;
            let surface = DibSurface::new(width_px, height_px)?;

            Ok(Self {
                hwnd,
                _d2d_factory: factories.d2d.clone(),
                target,
                brush,
                formats,
                surface,
                dpi: dpi_f,
            })
        }
    }

    pub fn resize(&mut self, width_px: u32, height_px: u32, dpi: u32) -> AppResult<()> {
        let width_px = width_px.max(1);
        let height_px = height_px.max(1);
        let dpi_f = dpi.max(96) as f32;

        if self.surface.width_px() != width_px || self.surface.height_px() != height_px {
            self.surface = DibSurface::new(width_px, height_px)?;
        }
        if (self.dpi - dpi_f).abs() > 0.1 {
            self.dpi = dpi_f;
            // SAFETY: Updating logical DPI on our owned `ID2D1DCRenderTarget`.
            unsafe {
                self.target.SetDpi(dpi_f, dpi_f);
            }
        }
        Ok(())
    }

    /// Renders into the 32-bit premultiplied DIB and blits to `paint_dc` (or acquires window DC if `None`).
    pub fn render(&mut self, view: &FlyoutRenderView<'_>, paint_dc: Option<HDC>) -> AppResult<()> {
        let bound_rect = RECT {
            left: 0,
            top: 0,
            right: self.surface.width_px() as i32,
            bottom: self.surface.height_px() as i32,
        };
        let open_t = view.open_progress.clamp(0.0, 1.0);
        let slide_dip = (1.0 - ease_out_quart(open_t)) * 14.0;
        let content_alpha = 0.15 + 0.85 * ease_out_cubic((open_t / 0.72).min(1.0));

        // SAFETY: Binding our owned memory DC, drawing inside `BeginDraw`/`EndDraw`, and blitting.
        unsafe {
            self.target
                .BindDC(self.surface.mem_dc(), &bound_rect)
                .map_err(|e| {
                    AppError::Backend(format!("ID2D1DCRenderTarget::BindDC failed: {e}"))
                })?;

            self.target.BeginDraw();
            self.target.SetTransform(&Matrix3x2::identity());
            self.target.Clear(Some(&to_d2d_color(Color::TRANSPARENT)));

            // Full-window subtle Acrylic tint stays fixed so rounded corners stay crisp during slide-up.
            let full = RectF::new(0.0, 0.0, view.geometry.width, view.geometry.height);
            self.fill_rect(full, view.palette.surface_tint);

            if slide_dip > 0.02 {
                self.target
                    .SetTransform(&Matrix3x2::translation(0.0, slide_dip));
            }
            self.draw_header(view, content_alpha);
            self.draw_rows(view, content_alpha);
            self.draw_footer(view, content_alpha);
            self.target.SetTransform(&Matrix3x2::identity());

            self.target
                .EndDraw(None, None)
                .map_err(|e| AppError::Backend(format!("D2D EndDraw failed: {e}")))?;

            self.present(paint_dc);
        }

        Ok(())
    }

    unsafe fn present(&self, paint_dc: Option<HDC>) {
        if let Some(hdc) = paint_dc {
            unsafe { self.blit_to_dc(hdc) };
        } else if let Some(guard) = ScreenDcGuard::acquire(Some(self.hwnd)) {
            unsafe { self.blit_to_dc(guard.raw()) };
        }
    }

    unsafe fn blit_to_dc(&self, dst_dc: HDC) {
        // SAFETY: `dst_dc` and `self.surface.mem_dc()` are valid DCs for the dimensions of `surface`.
        unsafe {
            let _ = BitBlt(
                dst_dc,
                0,
                0,
                self.surface.width_px() as i32,
                self.surface.height_px() as i32,
                self.surface.mem_dc(),
                0,
                0,
                SRCCOPY,
            );
        }
    }

    unsafe fn draw_header(&self, view: &FlyoutRenderView<'_>, alpha: f32) {
        let p = view.palette;
        let h = &view.geometry.header;

        unsafe {
            self.draw_text(
                "Keyboard layout",
                &self.formats.header_title,
                h.title,
                p.text_primary.scale_alpha(alpha),
            );

            // Native Windows 11 shortcut badges: [⊞] + [Spacebar]
            self.fill_rounded_rect(h.win_keycap, 4.0, p.keycap_bg.scale_alpha(alpha));
            self.stroke_rounded_rect(h.win_keycap, 4.0, p.keycap_border.scale_alpha(alpha), 1.0);
            self.draw_windows_logo_glyph(h.win_keycap, p.keycap_text.scale_alpha(alpha));

            self.draw_text(
                "+",
                &self.formats.keycap,
                h.plus_label,
                p.text_secondary.scale_alpha(alpha),
            );

            self.fill_rounded_rect(h.space_keycap, 4.0, p.keycap_bg.scale_alpha(alpha));
            self.stroke_rounded_rect(h.space_keycap, 4.0, p.keycap_border.scale_alpha(alpha), 1.0);
            self.draw_text(
                "Spacebar",
                &self.formats.keycap,
                h.space_keycap,
                p.keycap_text.scale_alpha(alpha),
            );
        }
    }

    unsafe fn draw_windows_logo_glyph(&self, cap: RectF, color: Color) {
        let cx = (cap.left + cap.right) * 0.5;
        let cy = (cap.top + cap.bottom) * 0.5;
        let cell = 4.2;
        let half_gap = 0.6;

        let panes = [
            RectF::new(
                cx - half_gap - cell,
                cy - half_gap - cell,
                cx - half_gap,
                cy - half_gap,
            ),
            RectF::new(
                cx + half_gap,
                cy - half_gap - cell,
                cx + half_gap + cell,
                cy - half_gap,
            ),
            RectF::new(
                cx - half_gap - cell,
                cy + half_gap,
                cx - half_gap,
                cy + half_gap + cell,
            ),
            RectF::new(
                cx + half_gap,
                cy + half_gap,
                cx + half_gap + cell,
                cy + half_gap + cell,
            ),
        ];
        unsafe {
            for pane in panes {
                self.fill_rect(pane, color);
            }
        }
    }

    unsafe fn draw_rows(&self, view: &FlyoutRenderView<'_>, alpha: f32) {
        let clip = to_d2d_rect(view.geometry.list_clip_rect);
        // SAFETY: Paired clip push/pop around scrollable list rows.
        unsafe {
            self.target
                .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);

            for (idx, (geom, item)) in view.geometry.rows.iter().zip(view.rows.iter()).enumerate() {
                if geom.card.bottom < view.geometry.list_clip_rect.top
                    || geom.card.top > view.geometry.list_clip_rect.bottom
                {
                    continue;
                }
                self.draw_row(idx, geom, item, view, alpha);
            }

            self.target.PopAxisAlignedClip();
        }
    }

    unsafe fn draw_row(
        &self,
        idx: usize,
        geom: &RowGeometry,
        item: &RowRenderItem<'_>,
        view: &FlyoutRenderView<'_>,
        alpha: f32,
    ) {
        let p = view.palette;
        let row_pressed = matches!(view.pressed, HitTarget::RowActivate(i) if i == idx);

        let bg = if row_pressed && item.is_enabled {
            p.row_pressed_bg
        } else if item.is_active {
            p.row_active_bg
                .lerp(p.row_active_hover_bg, item.row_hover_t)
        } else {
            Color::TRANSPARENT.lerp(p.row_hover_bg, item.row_hover_t)
        };

        unsafe {
            if bg.a > 0.002 {
                self.fill_rounded_rect(geom.card, ROW_RADIUS, bg.scale_alpha(alpha));
            }

            if view.focused_row == Some(idx) {
                self.stroke_rounded_rect(
                    geom.card,
                    ROW_RADIUS,
                    p.focus_ring.scale_alpha(alpha),
                    1.5,
                );
            }

            // Left vertical accent pill for the currently active input language.
            if item.is_active && item.is_enabled {
                self.fill_rounded_rect(geom.active_pill, 1.5, p.accent.scale_alpha(alpha));
            }

            // Smoothly interpolate text opacity alongside the toggle transition.
            let primary_color = p
                .text_disabled
                .lerp(p.text_primary, item.toggle_t)
                .scale_alpha(alpha);
            let secondary_color = p
                .text_secondary_disabled
                .lerp(p.text_secondary, item.toggle_t)
                .scale_alpha(alpha);

            self.draw_text(item.badge, &self.formats.badge, geom.badge, primary_color);
            self.draw_text(
                item.language_name,
                &self.formats.row_title,
                geom.title,
                primary_color,
            );
            self.draw_text(
                item.layout_name,
                &self.formats.row_subtitle,
                geom.subtitle,
                secondary_color,
            );

            self.draw_toggle_switch(geom.toggle_track, item, p, alpha);
        }
    }

    unsafe fn draw_toggle_switch(
        &self,
        track: RectF,
        item: &RowRenderItem<'_>,
        p: &FluentPalette,
        alpha: f32,
    ) {
        let t = item.toggle_t.clamp(0.0, 1.0);
        let hover_t = if item.is_locked_last {
            0.0
        } else {
            item.knob_hover_t.clamp(0.0, 1.0)
        };
        let press_t = if item.is_locked_last {
            0.0
        } else {
            item.knob_press_t.clamp(0.0, 1.0)
        };
        let radius = track.height() * 0.5;

        let on_fill = if item.is_locked_last {
            p.accent_locked
        } else {
            p.accent.lerp(p.accent_hover, hover_t)
        };
        let off_fill = p.toggle_off_bg.lerp(p.toggle_off_hover_bg, hover_t);

        let track_fill = off_fill.lerp(on_fill, t).scale_alpha(alpha);
        let border_color = p
            .toggle_off_border
            .scale_alpha((1.0 - t).clamp(0.0, 1.0) * alpha);

        let on_knob = if item.is_locked_last {
            p.toggle_locked_knob
        } else {
            p.toggle_on_knob
        };
        let knob_color = p.toggle_off_knob.lerp(on_knob, t).scale_alpha(alpha);

        // Authentic WinUI 3 Fluent 2 toggle thumb: circular at rest, grows smoothly on hover,
        // and stretches gently into a pill horizontally while pressed.
        let knob_r = 6.0 + hover_t * 1.0;
        let stretch_w = press_t * 3.5;
        let knob_w = knob_r * 2.0 + stretch_w;
        let knob_h = knob_r * 2.0;

        let inset = radius - knob_r;
        let min_left = track.left + inset;
        let max_left = track.right - inset - knob_w;
        let left = min_left + (max_left - min_left) * t;
        let top = (track.top + track.bottom - knob_h) * 0.5;
        let knob_rect = RectF::new(left, top, left + knob_w, top + knob_h);

        unsafe {
            self.fill_rounded_rect(track, radius, track_fill);
            if border_color.a > 0.01 {
                self.stroke_rounded_rect(track, radius, border_color, 1.0);
            }
            self.fill_rounded_rect(knob_rect, knob_r, knob_color);
        }
    }

    unsafe fn draw_footer(&self, view: &FlyoutRenderView<'_>, alpha: f32) {
        let p = view.palette;
        unsafe {
            self.fill_rect(view.geometry.footer_rect, p.footer_tint.scale_alpha(alpha));

            self.brush
                .SetColor(&to_d2d_color(p.divider.scale_alpha(alpha)));
            self.target.DrawLine(
                D2D_POINT_2F {
                    x: 0.0,
                    y: view.geometry.divider_y + 0.5,
                },
                D2D_POINT_2F {
                    x: view.geometry.width,
                    y: view.geometry.divider_y + 0.5,
                },
                &self.brush,
                1.0,
                None,
            );

            if let Some(notice) = view.status_notice {
                let notice_rect = view.geometry.footer_rect.inset_x(16.0);
                self.draw_text(
                    notice,
                    &self.formats.footer_link,
                    notice_rect,
                    p.accent.scale_alpha(alpha),
                );
                return;
            }

            self.draw_footer_button(
                &FooterButtonSpec {
                    rect: view.geometry.settings_button,
                    label: "More keyboard settings",
                    format: &self.formats.footer_link,
                    text_color: p.text_primary,
                    text_inset_x: 8.0,
                    hover_t: view.settings_hover_t,
                    is_pressed: view.pressed == HitTarget::FooterSettings,
                },
                p,
                alpha,
            );

            if let Some(restore_rect) = view.geometry.restore_button {
                self.draw_footer_button(
                    &FooterButtonSpec {
                        rect: restore_rect,
                        label: "Restore all",
                        format: &self.formats.footer_action,
                        text_color: p.accent,
                        text_inset_x: 6.0,
                        hover_t: view.restore_hover_t,
                        is_pressed: view.pressed == HitTarget::FooterRestoreAll,
                    },
                    p,
                    alpha,
                );
            }
        }
    }

    unsafe fn draw_footer_button(
        &self,
        spec: &FooterButtonSpec<'_>,
        p: &FluentPalette,
        alpha: f32,
    ) {
        let bg = if spec.is_pressed {
            p.row_pressed_bg
        } else {
            Color::TRANSPARENT.lerp(p.row_hover_bg, spec.hover_t)
        };
        unsafe {
            if bg.a > 0.002 {
                self.fill_rounded_rect(spec.rect, ROW_RADIUS, bg.scale_alpha(alpha));
            }
            self.draw_text(
                spec.label,
                spec.format,
                spec.rect.inset_x(spec.text_inset_x),
                spec.text_color.scale_alpha(alpha),
            );
        }
    }

    unsafe fn fill_rect(&self, rect: RectF, color: Color) {
        unsafe {
            self.brush.SetColor(&to_d2d_color(color));
            self.target.FillRectangle(&to_d2d_rect(rect), &self.brush);
        }
    }

    unsafe fn fill_rounded_rect(&self, rect: RectF, radius: f32, color: Color) {
        let rr = D2D1_ROUNDED_RECT {
            rect: to_d2d_rect(rect),
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            self.brush.SetColor(&to_d2d_color(color));
            self.target.FillRoundedRectangle(&rr, &self.brush);
        }
    }

    unsafe fn stroke_rounded_rect(&self, rect: RectF, radius: f32, color: Color, width: f32) {
        let half = width * 0.5;
        let inset = RectF::new(
            rect.left + half,
            rect.top + half,
            rect.right - half,
            rect.bottom - half,
        );
        let rr = D2D1_ROUNDED_RECT {
            rect: to_d2d_rect(inset),
            radiusX: (radius - half).max(0.0),
            radiusY: (radius - half).max(0.0),
        };
        unsafe {
            self.brush.SetColor(&to_d2d_color(color));
            self.target
                .DrawRoundedRectangle(&rr, &self.brush, width, None);
        }
    }

    unsafe fn draw_text(&self, text: &str, format: &IDWriteTextFormat, rect: RectF, color: Color) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let d2d_rect = to_d2d_rect(rect);
        unsafe {
            self.brush.SetColor(&to_d2d_color(color));
            self.target.DrawText(
                &wide,
                format,
                &d2d_rect,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
}

unsafe fn create_text_formats(dwrite: &IDWriteFactory) -> AppResult<TextFormats> {
    let make = |family: &str,
                weight: DWRITE_FONT_WEIGHT,
                size: f32,
                align: DWRITE_TEXT_ALIGNMENT,
                ellipsis: bool|
     -> AppResult<IDWriteTextFormat> {
        let wide_family = to_wide_null(family);
        let wide_locale = to_wide_null("");
        // SAFETY: Creating immutable DirectWrite text format objects.
        let fmt = unsafe {
            dwrite.CreateTextFormat(
                PCWSTR(wide_family.as_ptr()),
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                PCWSTR(wide_locale.as_ptr()),
            )
        }
        .map_err(|e| AppError::Backend(format!("CreateTextFormat failed: {e}")))?;

        unsafe {
            let _ = fmt.SetTextAlignment(align);
            let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            if ellipsis {
                let trimming = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                if let Ok(sign) = dwrite.CreateEllipsisTrimmingSign(&fmt) {
                    let _ = fmt.SetTrimming(&trimming, &sign);
                }
            }
        }
        Ok(fmt)
    };

    Ok(TextFormats {
        header_title: make(
            "Segoe UI Variable Text",
            DWRITE_FONT_WEIGHT_MEDIUM,
            14.0,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            false,
        )?,
        keycap: make(
            "Segoe UI Variable Small",
            DWRITE_FONT_WEIGHT_REGULAR,
            11.5,
            DWRITE_TEXT_ALIGNMENT_CENTER,
            false,
        )?,
        badge: make(
            "Segoe UI Variable Text",
            DWRITE_FONT_WEIGHT_MEDIUM,
            13.0,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            false,
        )?,
        row_title: make(
            "Segoe UI Variable Text",
            DWRITE_FONT_WEIGHT_REGULAR,
            14.0,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            true,
        )?,
        row_subtitle: make(
            "Segoe UI Variable Small",
            DWRITE_FONT_WEIGHT_REGULAR,
            12.0,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            true,
        )?,
        footer_link: make(
            "Segoe UI Variable Text",
            DWRITE_FONT_WEIGHT_REGULAR,
            13.0,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            true,
        )?,
        footer_action: make(
            "Segoe UI Variable Text",
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            12.5,
            DWRITE_TEXT_ALIGNMENT_TRAILING,
            false,
        )?,
    })
}
