//! Pure frame-rate-independent animation engine for the flyout (zero Win32 dependencies).

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::core::model::LayoutProfile;
use crate::ui::layout::HitTarget;

pub const OPEN_ANIM_DURATION_SECS: f32 = 0.22;

// Exponential ease-out rates (frame-rate independent at 60 / 120 / 144 / 240 Hz).
const TOGGLE_EXP_SPEED: f32 = 18.0;
const HOVER_EXP_SPEED: f32 = 22.0;
const PRESS_EXP_SPEED: f32 = 28.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowAnim {
    pub toggle_t: f32,
    pub row_hover_t: f32,
    pub knob_hover_t: f32,
    pub knob_press_t: f32,
}

impl RowAnim {
    pub const fn for_enabled(enabled: bool) -> Self {
        Self {
            toggle_t: if enabled { 1.0 } else { 0.0 },
            row_hover_t: 0.0,
            knob_hover_t: 0.0,
            knob_press_t: 0.0,
        }
    }

    pub fn reset_transient(&mut self) {
        self.row_hover_t = 0.0;
        self.knob_hover_t = 0.0;
        self.knob_press_t = 0.0;
    }
}

/// Manages all smooth Fluent 2 transitions (open slide-up, toggle thumb travel, hover fades,
/// and press elongation) with automatic idle convergence so the VSync loop sleeps at 0% CPU.
pub struct FlyoutAnimator {
    row_anims: HashMap<String, RowAnim>,
    settings_hover_t: f32,
    restore_hover_t: f32,
    open_started_at: Option<Instant>,
    last_frame_at: Instant,
    active: bool,
}

impl FlyoutAnimator {
    pub fn new(profiles: &[LayoutProfile], now: Instant) -> Self {
        let mut animator = Self {
            row_anims: HashMap::with_capacity(profiles.len()),
            settings_hover_t: 0.0,
            restore_hover_t: 0.0,
            open_started_at: None,
            last_frame_at: now,
            active: false,
        };
        animator.sync_profiles(profiles);
        animator
    }

    /// Inserts animation slots for newly discovered profiles and prunes removed ones.
    pub fn sync_profiles(&mut self, profiles: &[LayoutProfile]) {
        let live_ids: HashSet<&str> = profiles.iter().map(|p| p.tip_id.as_str()).collect();
        self.row_anims
            .retain(|id, _| live_ids.contains(id.as_str()));

        for p in profiles {
            self.row_anims
                .entry(p.tip_id.clone())
                .or_insert_with(|| RowAnim::for_enabled(p.enabled));
        }
    }

    pub fn on_open(&mut self, now: Instant) {
        self.settings_hover_t = 0.0;
        self.restore_hover_t = 0.0;
        for anim in self.row_anims.values_mut() {
            anim.reset_transient();
        }
        self.open_started_at = Some(now);
        self.last_frame_at = now;
        self.active = true;
    }

    pub fn on_hide(&mut self) {
        self.active = false;
        self.open_started_at = None;
    }

    pub fn request_frame(&mut self, now: Instant) {
        if !self.active {
            self.last_frame_at = now;
            self.active = true;
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn settings_hover_t(&self) -> f32 {
        self.settings_hover_t
    }

    pub fn restore_hover_t(&self) -> f32 {
        self.restore_hover_t
    }

    pub fn open_progress(&self, now: Instant) -> f32 {
        self.open_started_at
            .map(|started| {
                (now.saturating_duration_since(started).as_secs_f32() / OPEN_ANIM_DURATION_SECS)
                    .clamp(0.0, 1.0)
            })
            .unwrap_or(1.0)
    }

    pub fn row_anim(&self, profile: &LayoutProfile) -> RowAnim {
        self.row_anims
            .get(&profile.tip_id)
            .copied()
            .unwrap_or_else(|| RowAnim::for_enabled(profile.enabled))
    }

    /// Advances all animations by exact elapsed `dt` and returns whether another frame is needed.
    pub fn step(
        &mut self,
        now: Instant,
        profiles: &[LayoutProfile],
        hovered: HitTarget,
        pressed: HitTarget,
    ) -> bool {
        let dt = now
            .saturating_duration_since(self.last_frame_at)
            .as_secs_f32()
            .clamp(0.001, 0.05);
        self.last_frame_at = now;

        let mut still_animating = false;

        if let Some(started) = self.open_started_at {
            if now.saturating_duration_since(started).as_secs_f32() < OPEN_ANIM_DURATION_SECS {
                still_animating = true;
            } else {
                self.open_started_at = None;
            }
        }

        still_animating |= approach_bool(
            &mut self.settings_hover_t,
            hovered == HitTarget::FooterSettings,
            HOVER_EXP_SPEED,
            dt,
        );
        still_animating |= approach_bool(
            &mut self.restore_hover_t,
            hovered == HitTarget::FooterRestoreAll,
            HOVER_EXP_SPEED,
            dt,
        );

        for (idx, p) in profiles.iter().enumerate() {
            let entry = self
                .row_anims
                .entry(p.tip_id.clone())
                .or_insert_with(|| RowAnim::for_enabled(p.enabled));

            still_animating |= approach_bool(&mut entry.toggle_t, p.enabled, TOGGLE_EXP_SPEED, dt);

            let row_hovered = matches!(
                hovered,
                HitTarget::RowActivate(i) | HitTarget::RowToggle(i) if i == idx
            );
            still_animating |=
                approach_bool(&mut entry.row_hover_t, row_hovered, HOVER_EXP_SPEED, dt);
            still_animating |= approach_bool(
                &mut entry.knob_hover_t,
                hovered == HitTarget::RowToggle(idx),
                HOVER_EXP_SPEED,
                dt,
            );
            still_animating |= approach_bool(
                &mut entry.knob_press_t,
                pressed == HitTarget::RowToggle(idx),
                PRESS_EXP_SPEED,
                dt,
            );
        }

        self.active = still_animating;
        still_animating
    }
}

pub fn ease_out_cubic(x: f32) -> f32 {
    let inv = 1.0 - x.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

pub fn ease_out_quart(x: f32) -> f32 {
    let inv = 1.0 - x.clamp(0.0, 1.0);
    let sq = inv * inv;
    1.0 - sq * sq
}

/// Frame-rate-independent exponential ease-out (`1 - e^(-speed * dt)`).
pub fn exp_approach(current: &mut f32, target: f32, speed: f32, dt: f32) -> bool {
    let diff = target - *current;
    if diff.abs() < 0.002 {
        *current = target;
        false
    } else {
        let factor = 1.0 - (-speed * dt).exp();
        *current += diff * factor;
        true
    }
}

fn approach_bool(current: &mut f32, flag: bool, speed: f32, dt: f32) -> bool {
    exp_approach(current, if flag { 1.0 } else { 0.0 }, speed, dt)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::core::model::ProfileKind;

    fn make_profile(tip: &str, enabled: bool) -> LayoutProfile {
        LayoutProfile {
            tip_id: tip.into(),
            kind: ProfileKind::KeyboardLayout,
            langid: 0x0409,
            clsid: None,
            guid_profile: None,
            bcp47: "en-US".into(),
            short_badge: "ENG".into(),
            language_name: "English".into(),
            layout_name: "US".into(),
            enabled,
        }
    }

    #[test]
    fn sync_profiles_prunes_removed_entries_and_adds_new_ones() {
        let now = Instant::now();
        let initial = vec![make_profile("A", true), make_profile("B", false)];
        let mut anim = FlyoutAnimator::new(&initial, now);

        let updated = vec![make_profile("B", false), make_profile("C", true)];
        anim.sync_profiles(&updated);

        assert!(!anim.row_anims.contains_key("A"));
        assert_eq!(anim.row_anims["B"].toggle_t, 0.0);
        assert_eq!(anim.row_anims["C"].toggle_t, 1.0);
    }

    #[test]
    fn step_converges_toggle_and_open_animation_to_idle() {
        let t0 = Instant::now();
        let mut profiles = vec![make_profile("A", true)];
        let mut anim = FlyoutAnimator::new(&profiles, t0);
        anim.on_open(t0);
        assert!(anim.is_active());

        // Flip profile OFF and advance at 60 Hz for 1 second.
        profiles[0].enabled = false;
        let mut t = t0;
        for _ in 0..60 {
            t += Duration::from_millis(16);
            anim.step(t, &profiles, HitTarget::None, HitTarget::None);
        }

        assert!(!anim.is_active());
        assert_eq!(anim.row_anim(&profiles[0]).toggle_t, 0.0);
        assert_eq!(anim.open_progress(t), 1.0);
    }

    #[test]
    fn easing_functions_clamp_endpoints() {
        assert_eq!(ease_out_cubic(-1.0), 0.0);
        assert_eq!(ease_out_cubic(2.0), 1.0);
        assert_eq!(ease_out_quart(0.0), 0.0);
        assert_eq!(ease_out_quart(1.0), 1.0);
    }
}
