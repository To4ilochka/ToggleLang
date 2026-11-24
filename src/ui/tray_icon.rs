use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_COMBINE_MODE_EXCLUDE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_ROUNDED_RECT,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, ID2D1RenderTarget,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad};
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::WindowsAndMessaging::{HICON, SM_CXSMICON};
use windows::core::PCWSTR;
use windows_numerics::{Matrix3x2, Vector2};

use crate::core::error::{AppError, AppResult};
use crate::core::model::TrayIconMode;
use crate::platform::win_util::to_wide_null;
use crate::ui::graphics::{GraphicsFactories, bgra_pixels_to_hicon};

/// Synthesizes DPI-crisp 32-bit premultiplied tray `HICON`s for each `TrayIconMode` strategy
/// (Fluent stacked toggle vector icon vs. rounded keycap language code).
pub struct TrayIconRenderer {
    factories: GraphicsFactories,
}

impl TrayIconRenderer {
    pub fn new(factories: GraphicsFactories) -> Self {
        Self { factories }
    }

    pub fn render_hicon(
        &self,
        mode: TrayIconMode,
        keycap_code: &str,
        taskbar_dark: bool,
        dpi: u32,
    ) -> AppResult<HICON> {
        // SAFETY: Querying system small icon metric for the active monitor DPI.
        let metric_px = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
        let size = (metric_px.max(16) as u32).clamp(16, 64);

        let pixels = self.render_pixels(mode, keycap_code, taskbar_dark, size)?;
        bgra_pixels_to_hicon(&pixels, size)
    }

    fn render_pixels(
        &self,
        mode: TrayIconMode,
        keycap_code: &str,
        taskbar_dark: bool,
        size: u32,
    ) -> AppResult<Vec<u8>> {
        // SAFETY: Creating a 32-bit premultiplied BGRA WIC bitmap and D2D render target over it.
        unsafe {
            let wic_bitmap = self
                .factories
                .wic
                .CreateBitmap(
                    size,
                    size,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapCacheOnLoad,
                )
                .map_err(|e| AppError::Backend(format!("WIC CreateBitmap failed: {e}")))?;

            let rt_props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };

            let target = self
                .factories
                .d2d
                .CreateWicBitmapRenderTarget(&wic_bitmap, &rt_props)
                .map_err(|e| {
                    AppError::Backend(format!("CreateWicBitmapRenderTarget failed: {e}"))
                })?;

            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            target.BeginDraw();
            target.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));

            let draw_result = match mode {
                TrayIconMode::VectorIcon => {
                    self.draw_vector_toggle_icon(&target, size, taskbar_dark)
                }
                TrayIconMode::KeycapCode => {
                    self.draw_keycap_code_icon(&target, size, keycap_code, taskbar_dark)
                }
            };

            let end_result = target
                .EndDraw(None, None)
                .map_err(|e| AppError::Backend(format!("EndDraw(tray) failed: {e}")));

            draw_result?;
            end_result?;

            let stride = size * 4;
            let mut pixels = vec![0u8; (stride * size) as usize];
            wic_bitmap
                .CopyPixels(std::ptr::null(), stride, &mut pixels)
                .map_err(|e| AppError::Backend(format!("WIC CopyPixels failed: {e}")))?;
            Ok(pixels)
        }
    }

    /// Strategy 1 (Default): Minimalist Windows 11 Fluent stacked toggle switches.
    /// Top toggle is ON (solid pill with transparent alpha knob cutout via `D2D1_COMBINE_MODE_EXCLUDE`);
    /// bottom toggle is OFF (crisp outlined pill with left knob).
    unsafe fn draw_vector_toggle_icon(
        &self,
        target: &ID2D1RenderTarget,
        size: u32,
        taskbar_dark: bool,
    ) -> AppResult<()> {
        let s = size as f32 / 16.0;
        let fg = tray_foreground_color(taskbar_dark);

        unsafe {
            let brush = target.CreateSolidColorBrush(&fg, None).map_err(|e| {
                AppError::Backend(format!("CreateSolidColorBrush(vec) failed: {e}"))
            })?;

            let top_rr = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: 0.75 * s,
                    top: 1.5 * s,
                    right: 15.25 * s,
                    bottom: 7.0 * s,
                },
                radiusX: 2.75 * s,
                radiusY: 2.75 * s,
            };
            let top_knob = D2D1_ELLIPSE {
                point: Vector2 {
                    X: 12.5 * s,
                    Y: 4.25 * s,
                },
                radiusX: 1.45 * s,
                radiusY: 1.45 * s,
            };

            let pill_geo = self
                .factories
                .d2d
                .CreateRoundedRectangleGeometry(&top_rr)
                .map_err(|e| AppError::Backend(format!("CreateRoundedRectangleGeometry: {e}")))?;
            let hole_geo = self
                .factories
                .d2d
                .CreateEllipseGeometry(&top_knob)
                .map_err(|e| AppError::Backend(format!("CreateEllipseGeometry: {e}")))?;
            let combined = self
                .factories
                .d2d
                .CreatePathGeometry()
                .map_err(|e| AppError::Backend(format!("CreatePathGeometry: {e}")))?;
            let sink = combined
                .Open()
                .map_err(|e| AppError::Backend(format!("PathGeometry::Open: {e}")))?;
            pill_geo
                .CombineWithGeometry(&hole_geo, D2D1_COMBINE_MODE_EXCLUDE, None, 0.1, &sink)
                .map_err(|e| AppError::Backend(format!("CombineWithGeometry: {e}")))?;
            sink.Close()
                .map_err(|e| AppError::Backend(format!("GeometrySink::Close: {e}")))?;

            target.FillGeometry(&combined, &brush, None);

            let stroke = (1.25 * s).max(1.0);
            let half = stroke * 0.5;
            let bot_rr = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: 0.75 * s + half,
                    top: 9.0 * s + half,
                    right: 15.25 * s - half,
                    bottom: 14.5 * s - half,
                },
                radiusX: (2.75 * s - half).max(1.0),
                radiusY: (2.75 * s - half).max(1.0),
            };
            target.DrawRoundedRectangle(&bot_rr, &brush, stroke, None);

            let bot_knob = D2D1_ELLIPSE {
                point: Vector2 {
                    X: 3.6 * s,
                    Y: 11.75 * s,
                },
                radiusX: 1.35 * s,
                radiusY: 1.35 * s,
            };
            target.FillEllipse(&bot_knob, &brush);
        }

        Ok(())
    }

    /// Strategy 2: Crisp Windows 11 keycap displaying 2-letter uppercase language code (`EN`, `RU`, `UK`, `日`).
    unsafe fn draw_keycap_code_icon(
        &self,
        target: &ID2D1RenderTarget,
        size: u32,
        code: &str,
        taskbar_dark: bool,
    ) -> AppResult<()> {
        let s = size as f32 / 16.0;
        let fg = tray_foreground_color(taskbar_dark);
        let fill = if taskbar_dark {
            D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.10,
            }
        } else {
            D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.07,
            }
        };
        let border = if taskbar_dark {
            D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.80,
            }
        } else {
            D2D1_COLOR_F {
                r: 0.08,
                g: 0.08,
                b: 0.08,
                a: 0.78,
            }
        };

        unsafe {
            let fill_brush = target
                .CreateSolidColorBrush(&fill, None)
                .map_err(|e| AppError::Backend(format!("CreateSolidColorBrush(fill): {e}")))?;
            let border_brush = target
                .CreateSolidColorBrush(&border, None)
                .map_err(|e| AppError::Backend(format!("CreateSolidColorBrush(border): {e}")))?;
            let text_brush = target
                .CreateSolidColorBrush(&fg, None)
                .map_err(|e| AppError::Backend(format!("CreateSolidColorBrush(text): {e}")))?;

            let stroke = (1.15 * s).max(1.0);
            let half = stroke * 0.5;
            let keycap = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: 0.5 * s + half,
                    top: 0.5 * s + half,
                    right: 15.5 * s - half,
                    bottom: 15.5 * s - half,
                },
                radiusX: 2.8 * s,
                radiusY: 2.8 * s,
            };
            target.FillRoundedRectangle(&keycap, &fill_brush);
            target.DrawRoundedRectangle(&keycap, &border_brush, stroke, None);

            let (font_size, scale_x) = match code.chars().count() {
                0 | 1 => (size as f32 * 0.64, 1.0_f32),
                2 => (size as f32 * 0.54, 0.90_f32),
                _ => (size as f32 * 0.44, 0.78_f32),
            };

            let family = to_wide_null("Segoe UI Variable Display");
            let locale = to_wide_null("");
            let format = self
                .factories
                .dwrite
                .CreateTextFormat(
                    PCWSTR(family.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    font_size,
                    PCWSTR(locale.as_ptr()),
                )
                .map_err(|e| AppError::Backend(format!("CreateTextFormat(keycap): {e}")))?;
            let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);

            let wide_text: Vec<u16> = code.encode_utf16().collect();
            let logical_w = size as f32 / scale_x;
            let text_rect = D2D_RECT_F {
                left: 0.0,
                top: -0.3 * s,
                right: logical_w,
                bottom: size as f32 - 0.3 * s,
            };

            target.SetTransform(&Matrix3x2 {
                M11: scale_x,
                M12: 0.0,
                M21: 0.0,
                M22: 1.0,
                M31: 0.0,
                M32: 0.0,
            });
            target.DrawText(
                &wide_text,
                &format,
                &text_rect,
                &text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            target.SetTransform(&Matrix3x2::identity());
        }

        Ok(())
    }
}

fn tray_foreground_color(taskbar_dark: bool) -> D2D1_COLOR_F {
    if taskbar_dark {
        D2D1_COLOR_F {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.96,
        }
    } else {
        D2D1_COLOR_F {
            r: 0.08,
            g: 0.08,
            b: 0.08,
            a: 0.94,
        }
    }
}
