//! Pure hover, press, keyboard focus, and scroll state machine (zero Win32 dependencies).

use crate::ui::layout::HitTarget;

const SCROLL_STEP_DIP: f32 = 36.0;

#[derive(Debug, Default)]
pub struct FlyoutInteractionState {
    pub hovered: HitTarget,
    pub pressed: HitTarget,
    pub focused_row: Option<usize>,
    pub scroll_offset: f32,
}

impl FlyoutInteractionState {
    pub fn reset_transient(&mut self) {
        self.hovered = HitTarget::None;
        self.pressed = HitTarget::None;
        self.focused_row = None;
    }

    pub fn on_hover(&mut self, target: HitTarget) -> bool {
        if target != self.hovered {
            self.hovered = target;
            true
        } else {
            false
        }
    }

    pub fn on_mouse_leave(&mut self) -> bool {
        if self.hovered != HitTarget::None || self.pressed != HitTarget::None {
            self.hovered = HitTarget::None;
            self.pressed = HitTarget::None;
            true
        } else {
            false
        }
    }

    pub fn on_press(&mut self, target: HitTarget) {
        self.pressed = target;
    }

    /// Commits a click only when the release target matches the original press target.
    pub fn on_release(&mut self, target: HitTarget) -> Option<HitTarget> {
        let was_pressed = self.pressed;
        self.pressed = HitTarget::None;
        (target == was_pressed && target != HitTarget::None).then_some(target)
    }

    pub fn on_scroll(&mut self, wheel_notches: f32, max_scroll: f32) -> bool {
        if max_scroll <= 0.0 {
            return false;
        }
        let next = (self.scroll_offset - wheel_notches * SCROLL_STEP_DIP).clamp(0.0, max_scroll);
        if (next - self.scroll_offset).abs() > f32::EPSILON {
            self.scroll_offset = next;
            true
        } else {
            false
        }
    }

    pub fn move_focus_down(&mut self, count: usize, active_index: Option<usize>) -> bool {
        if count == 0 {
            return false;
        }
        self.focused_row = Some(match self.focused_row {
            Some(i) => (i + 1) % count,
            None => active_index.unwrap_or(0).min(count - 1),
        });
        true
    }

    pub fn move_focus_up(&mut self, count: usize, active_index: Option<usize>) -> bool {
        if count == 0 {
            return false;
        }
        self.focused_row = Some(match self.focused_row {
            Some(0) => count - 1,
            Some(i) => i - 1,
            None => active_index.unwrap_or(0).min(count - 1),
        });
        true
    }

    pub fn selected_row(&self, active_index: Option<usize>) -> Option<usize> {
        self.focused_row.or(active_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commits_click_only_when_press_and_release_targets_match() {
        let mut state = FlyoutInteractionState::default();
        state.on_press(HitTarget::RowToggle(1));
        assert_eq!(state.on_release(HitTarget::RowActivate(1)), None);
        assert_eq!(state.pressed, HitTarget::None);

        state.on_press(HitTarget::RowToggle(1));
        assert_eq!(
            state.on_release(HitTarget::RowToggle(1)),
            Some(HitTarget::RowToggle(1))
        );
    }

    #[test]
    fn keyboard_focus_wraps_cleanly_in_both_directions() {
        let mut state = FlyoutInteractionState::default();
        assert!(state.move_focus_down(3, Some(1)));
        assert_eq!(state.focused_row, Some(1));
        assert!(state.move_focus_down(3, Some(1)));
        assert_eq!(state.focused_row, Some(2));
        assert!(state.move_focus_down(3, Some(1)));
        assert_eq!(state.focused_row, Some(0));
        assert!(state.move_focus_up(3, Some(1)));
        assert_eq!(state.focused_row, Some(2));
    }

    #[test]
    fn clamps_scroll_within_bounds() {
        let mut state = FlyoutInteractionState::default();
        assert!(!state.on_scroll(-1.0, 0.0));
        assert!(state.on_scroll(-2.0, 50.0));
        assert_eq!(state.scroll_offset, 50.0);
        assert!(state.on_scroll(10.0, 50.0));
        assert_eq!(state.scroll_offset, 0.0);
    }
}
