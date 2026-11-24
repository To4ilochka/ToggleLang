//! Pure DIP geometry & hit-testing for the Windows 11 Keyboard Layout Flyout (zero Win32 calls).
//!
//! Acts as the single source of truth for every element rectangle rendered in `ui::renderer`.

pub const FLYOUT_WIDTH: f32 = 348.0;
pub const HEADER_HEIGHT: f32 = 44.0;
pub const LIST_TOP_PAD: f32 = 4.0;
pub const LIST_BOTTOM_PAD: f32 = 8.0;
pub const ROW_HEIGHT: f32 = 56.0;
pub const ROW_GAP: f32 = 4.0;
pub const ROW_INSET_X: f32 = 8.0;
pub const ROW_RADIUS: f32 = 6.0;
pub const FOOTER_HEIGHT: f32 = 46.0;
pub const MAX_VISIBLE_ROWS: usize = 8;

pub const TOGGLE_WIDTH: f32 = 40.0;
pub const TOGGLE_HEIGHT: f32 = 20.0;
pub const TOGGLE_RIGHT_MARGIN: f32 = 14.0;
pub const TOGGLE_HIT_EXTRA_LEFT: f32 = 12.0;

pub const FLYOUT_SCREEN_MARGIN_DIP: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectF {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl RectF {
    pub const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn height(self) -> f32 {
        self.bottom - self.top
    }

    pub fn inset_x(self, dx: f32) -> Self {
        Self {
            left: self.left + dx,
            right: self.right - dx,
            ..self
        }
    }

    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HitTarget {
    #[default]
    None,
    RowActivate(usize),
    RowToggle(usize),
    FooterSettings,
    FooterRestoreAll,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeaderGeometry {
    pub bounds: RectF,
    pub title: RectF,
    pub win_keycap: RectF,
    pub plus_label: RectF,
    pub space_keycap: RectF,
}

#[derive(Debug, Clone)]
pub struct RowGeometry {
    pub card: RectF,
    pub active_pill: RectF,
    pub badge: RectF,
    pub title: RectF,
    pub subtitle: RectF,
    pub toggle_track: RectF,
    pub toggle_hit: RectF,
}

#[derive(Debug, Clone)]
pub struct FlyoutGeometry {
    pub width: f32,
    pub height: f32,
    pub header: HeaderGeometry,
    pub list_clip_rect: RectF,
    pub rows: Vec<RowGeometry>,
    pub divider_y: f32,
    pub footer_rect: RectF,
    pub settings_button: RectF,
    pub restore_button: Option<RectF>,
    pub max_scroll: f32,
}

impl FlyoutGeometry {
    pub fn compute(item_count: usize, has_disabled: bool, scroll_offset: f32) -> Self {
        let visible_rows = item_count.clamp(1, MAX_VISIBLE_ROWS);
        let content_list_height = list_height_for(item_count);
        let visible_list_height = list_height_for(visible_rows);
        let max_scroll = (content_list_height - visible_list_height).max(0.0);
        let scroll = scroll_offset.clamp(0.0, max_scroll);

        let list_top = HEADER_HEIGHT;
        let list_bottom = list_top + visible_list_height;
        let total_height = list_bottom + FOOTER_HEIGHT;

        let header = HeaderGeometry {
            bounds: RectF::new(0.0, 0.0, FLYOUT_WIDTH, HEADER_HEIGHT),
            title: RectF::new(16.0, 0.0, 140.0, HEADER_HEIGHT),
            win_keycap: RectF::new(134.0, 12.0, 158.0, 32.0),
            plus_label: RectF::new(159.0, 11.0, 171.0, 33.0),
            space_keycap: RectF::new(172.0, 12.0, 234.0, 32.0),
        };

        let mut rows = Vec::with_capacity(item_count);
        for i in 0..item_count {
            let y = list_top + LIST_TOP_PAD + (i as f32) * (ROW_HEIGHT + ROW_GAP) - scroll;
            let card = RectF::new(ROW_INSET_X, y, FLYOUT_WIDTH - ROW_INSET_X, y + ROW_HEIGHT);

            let pill_h = 18.0;
            let pill_y = y + (ROW_HEIGHT - pill_h) * 0.5;
            let active_pill = RectF::new(card.left, pill_y, card.left + 3.0, pill_y + pill_h);

            let toggle_right = card.right - TOGGLE_RIGHT_MARGIN;
            let toggle_left = toggle_right - TOGGLE_WIDTH;
            let toggle_top = y + (ROW_HEIGHT - TOGGLE_HEIGHT) * 0.5;
            let toggle_track = RectF::new(
                toggle_left,
                toggle_top,
                toggle_right,
                toggle_top + TOGGLE_HEIGHT,
            );
            let toggle_hit = RectF::new(
                toggle_left - TOGGLE_HIT_EXTRA_LEFT,
                card.top,
                card.right,
                card.bottom,
            );

            let badge = RectF::new(card.left + 12.0, y + 10.0, card.left + 56.0, y + 32.0);
            let text_left = card.left + 58.0;
            let text_right = toggle_left - 12.0;
            let title = RectF::new(text_left, y + 9.0, text_right, y + 31.0);
            let subtitle = RectF::new(text_left, y + 31.0, text_right, y + 49.0);

            rows.push(RowGeometry {
                card,
                active_pill,
                badge,
                title,
                subtitle,
                toggle_track,
                toggle_hit,
            });
        }

        let footer_rect = RectF::new(0.0, list_bottom, FLYOUT_WIDTH, total_height);
        let restore_button = has_disabled.then(|| {
            RectF::new(
                FLYOUT_WIDTH - 104.0,
                list_bottom + 7.0,
                FLYOUT_WIDTH - 8.0,
                total_height - 7.0,
            )
        });
        let settings_right = if has_disabled {
            FLYOUT_WIDTH - 110.0
        } else {
            FLYOUT_WIDTH - 8.0
        };
        let settings_button =
            RectF::new(8.0, list_bottom + 7.0, settings_right, total_height - 7.0);

        Self {
            width: FLYOUT_WIDTH,
            height: total_height,
            header,
            list_clip_rect: RectF::new(0.0, list_top, FLYOUT_WIDTH, list_bottom),
            rows,
            divider_y: list_bottom,
            footer_rect,
            settings_button,
            restore_button,
            max_scroll,
        }
    }

    pub fn hit_test(&self, x: f32, y: f32) -> HitTarget {
        if self.list_clip_rect.contains(x, y) {
            for (idx, row) in self.rows.iter().enumerate() {
                if row.toggle_hit.contains(x, y) {
                    return HitTarget::RowToggle(idx);
                }
                if row.card.contains(x, y) {
                    return HitTarget::RowActivate(idx);
                }
            }
        }
        if let Some(restore) = self.restore_button
            && restore.contains(x, y)
        {
            return HitTarget::FooterRestoreAll;
        }
        if self.settings_button.contains(x, y) {
            return HitTarget::FooterSettings;
        }
        HitTarget::None
    }
}

fn list_height_for(count: usize) -> f32 {
    if count == 0 {
        return LIST_TOP_PAD + ROW_HEIGHT + LIST_BOTTOM_PAD;
    }
    LIST_TOP_PAD + (count as f32) * ROW_HEIGHT + ((count - 1) as f32) * ROW_GAP + LIST_BOTTOM_PAD
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Computes the physical pixel top-left position `(x, y)` for the flyout anchored near `(anchor_x, anchor_y)`
/// and clamped cleanly inside `work_area` with a 12 DIP Windows 11 gap.
pub fn compute_flyout_position(
    anchor_x: i32,
    anchor_y: i32,
    work_area: ScreenRect,
    width_px: i32,
    height_px: i32,
    dpi: u32,
) -> (i32, i32) {
    let margin = ((FLYOUT_SCREEN_MARGIN_DIP * dpi as f32) / 96.0).round() as i32;

    // Horizontally center over the tray icon anchor, then clamp inside the monitor work area.
    let ideal_x = anchor_x - width_px / 2;
    let min_x = work_area.left + margin;
    let max_x = (work_area.right - margin - width_px).max(min_x);
    let x = ideal_x.clamp(min_x, max_x);

    // Vertically place above the bottom taskbar (or below if taskbar is docked at top).
    let work_mid_y = work_area.top + (work_area.bottom - work_area.top) / 2;
    let ideal_y = if anchor_y >= work_mid_y {
        work_area.bottom - margin - height_px
    } else {
        work_area.top + margin
    };
    let min_y = work_area.top + margin;
    let max_y = (work_area.bottom - margin - height_px).max(min_y);
    let y = ideal_y.clamp(min_y, max_y);

    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_tests_row_activate_vs_right_side_toggle() {
        let geom = FlyoutGeometry::compute(3, true, 0.0);
        let row1 = &geom.rows[1];

        let left_x = row1.card.left + 40.0;
        let mid_y = (row1.card.top + row1.card.bottom) * 0.5;
        assert_eq!(geom.hit_test(left_x, mid_y), HitTarget::RowActivate(1));

        let toggle_x = (row1.toggle_track.left + row1.toggle_track.right) * 0.5;
        assert_eq!(geom.hit_test(toggle_x, mid_y), HitTarget::RowToggle(1));

        let settings = geom.settings_button;
        assert_eq!(
            geom.hit_test(settings.left + 10.0, settings.top + 10.0),
            HitTarget::FooterSettings
        );
        let restore = geom.restore_button.unwrap();
        assert_eq!(
            geom.hit_test(restore.left + 10.0, restore.top + 10.0),
            HitTarget::FooterRestoreAll
        );
    }

    #[test]
    fn computes_scrollable_height_when_exceeding_max_visible_rows() {
        let small = FlyoutGeometry::compute(MAX_VISIBLE_ROWS, false, 0.0);
        let large = FlyoutGeometry::compute(MAX_VISIBLE_ROWS + 3, false, 9999.0);
        assert_eq!(small.max_scroll, 0.0);
        assert!(large.max_scroll > 0.0);
        assert_eq!(small.height, large.height);
        // Clamped scroll aligns last row bottom cleanly inside list clip rect.
        let last = large.rows.last().unwrap();
        assert!((last.card.bottom + LIST_BOTTOM_PAD - large.list_clip_rect.bottom).abs() < 1e-3);
    }

    #[test]
    fn handles_empty_profile_list_without_underflow() {
        let empty = FlyoutGeometry::compute(0, false, 0.0);
        assert!(empty.rows.is_empty());
        assert_eq!(empty.max_scroll, 0.0);
        assert!(empty.restore_button.is_none());
    }

    #[test]
    fn flyout_position_clamps_inside_work_area_bottom_and_top() {
        let work = ScreenRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1032,
        };
        // Bottom-right taskbar anchor
        let (x, y) = compute_flyout_position(1850, 1060, work, 348, 280, 96);
        assert_eq!(x, 1920 - 12 - 348);
        assert_eq!(y, 1032 - 12 - 280);

        // Top-docked taskbar anchor
        let (x_top, y_top) = compute_flyout_position(100, 10, work, 348, 280, 96);
        assert_eq!(x_top, 12);
        assert_eq!(y_top, 12);
    }
}
