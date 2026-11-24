use crate::core::error::{AppError, AppResult};
use crate::core::model::{AppState, LayoutProfile, SyncReport, TrayIconMode};
use crate::core::plan::ApplyPlan;
use crate::core::ports::{InputBackend, StateRepository};

/// Domain coordinator managing layout visibility, ordering, reconciliation, and safety invariants.
///
/// Depends strictly on `InputBackend` and `StateRepository` ports (Dependency Inversion Principle).
pub struct LayoutManager<B, S> {
    backend: B,
    repo: S,
    state: AppState,
}

impl<B: InputBackend, S: StateRepository> LayoutManager<B, S> {
    pub fn open(backend: B, repo: S) -> AppResult<Self> {
        let state = repo.load()?.unwrap_or_default();
        let mut mgr = Self {
            backend,
            repo,
            state,
        };
        let _ = mgr.sync_with_system()?;
        Ok(mgr)
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn state_location(&self) -> String {
        self.repo.state_location()
    }

    pub fn recovery_location(&self) -> Option<String> {
        self.repo.recovery_location()
    }

    pub fn enabled_count(&self) -> usize {
        self.state.enabled_count()
    }

    pub fn has_disabled_profiles(&self) -> bool {
        self.state.has_disabled_profiles()
    }

    pub fn keycap_code_for(&self, active_idx: Option<usize>) -> String {
        self.state.keycap_code_for(active_idx)
    }

    pub fn tray_icon_mode(&self) -> TrayIconMode {
        self.state.tray_icon_mode
    }

    /// Updates and atomically persists the tray icon appearance preference.
    pub fn set_tray_icon_mode(&mut self, mode: TrayIconMode) -> AppResult<()> {
        if self.state.tray_icon_mode == mode {
            return Ok(());
        }
        self.state.tray_icon_mode = mode;
        self.repo.save(&self.state)
    }

    /// Reconciles persisted state against currently active OS layouts.
    pub fn sync_with_system(&mut self) -> AppResult<SyncReport> {
        let live_profiles = self.backend.list_enabled()?;
        let mut report = SyncReport::default();

        if self.state.profiles.is_empty() {
            self.state.original_snapshot = live_profiles.iter().map(|p| p.tip_id.clone()).collect();
            self.state.profiles = live_profiles;
            self.repo.save(&self.state)?;
            return Ok(report);
        }

        let mut changed = self.reconcile_existing_profiles(&live_profiles, &mut report);
        changed |= self.append_new_live_profiles(live_profiles, &mut report);

        if changed {
            self.repo.save(&self.state)?;
        }

        Ok(report)
    }

    /// Returns the index in `self.state.profiles` of the currently active profile, if resolvable.
    pub fn active_profile_index(&self) -> Option<usize> {
        let active_tip = self.backend.active_tip_id(&self.state.profiles)?;
        self.state
            .profiles
            .iter()
            .position(|p| p.enabled && p.tip_id.eq_ignore_ascii_case(&active_tip))
    }

    /// Enables or disables a profile selected by 1-based index, TIP ID, BCP-47 tag, or badge.
    pub fn set_enabled(
        &mut self,
        selector: &str,
        target_enabled: bool,
    ) -> AppResult<LayoutProfile> {
        let idx = self.resolve_selector(selector)?;
        self.apply_enabled_index(idx, target_enabled)
    }

    pub fn toggle(&mut self, selector: &str) -> AppResult<LayoutProfile> {
        let idx = self.resolve_selector(selector)?;
        let next = !self.state.profiles[idx].enabled;
        self.apply_enabled_index(idx, next)
    }

    /// Optimistically toggles a profile in memory and returns an `ApplyPlan` so the UI can animate
    /// immediately at 60–240 Hz while a background worker applies the change to Windows.
    pub fn plan_toggle(
        &mut self,
        selector: &str,
        current_active_idx: Option<usize>,
    ) -> AppResult<(LayoutProfile, ApplyPlan)> {
        let idx = self.resolve_selector(selector)?;
        let target_enabled = !self.state.profiles[idx].enabled;
        let plan = self
            .plan_set_enabled_index(idx, target_enabled, current_active_idx)?
            .expect("toggling always changes enabled state");
        Ok((self.state.profiles[idx].clone(), plan))
    }

    /// Optimistically enables all tracked layouts in memory and returns an `ApplyPlan`.
    pub fn plan_restore_all(&mut self, current_active_idx: Option<usize>) -> Option<ApplyPlan> {
        if self.state.profiles.is_empty() || !self.has_disabled_profiles() {
            return None;
        }
        let prev_profiles = self.state.profiles.clone();
        let prev_active_tips = self.state.enabled_tip_ids();
        let restore_active_after =
            current_active_idx.and_then(|i| self.state.profiles.get(i).cloned());

        for p in &mut self.state.profiles {
            p.enabled = true;
        }
        let new_active_tips = self.state.enabled_tip_ids();

        Some(ApplyPlan {
            prev_profiles,
            prev_active_tips,
            new_active_tips,
            switch_away_before: None,
            restore_active_after,
            new_state: self.state.clone(),
        })
    }

    /// Reverts optimistic in-memory state if an asynchronous OS apply failed and rolled back.
    pub fn revert_plan(&mut self, prev_profiles: Vec<LayoutProfile>) {
        self.state.profiles = prev_profiles;
    }

    /// Switches the active input profile for the current session.
    pub fn activate(&self, selector: &str) -> AppResult<LayoutProfile> {
        let idx = self.resolve_selector(selector)?;
        let profile = &self.state.profiles[idx];
        if !profile.enabled {
            return Err(AppError::ProfileDisabled(profile.tip_id.clone()));
        }
        self.backend.activate_profile(profile)?;
        Ok(profile.clone())
    }

    /// Re-enables every tracked layout in canonical order.
    pub fn restore_all(&mut self) -> AppResult<usize> {
        if self.state.profiles.is_empty() {
            return Ok(0);
        }

        let current_active_idx = self.active_profile_index();
        if let Some(plan) = self.plan_restore_all(current_active_idx) {
            self.execute_plan_or_revert(plan)?;
        } else {
            // All layouts are already enabled in state; ensure the OS matches canonical order.
            let all_tips = self.state.enabled_tip_ids();
            self.backend.apply_enabled_order(&all_tips)?;
            if let Some(active) =
                current_active_idx.and_then(|i| self.state.profiles.get(i).cloned())
            {
                let _ = self.backend.activate_profile(&active);
            }
            self.repo.save(&self.state)?;
        }

        Ok(self.state.profiles.len())
    }

    /// Re-applies the persisted `enabled` subset to Windows (e.g. on session startup).
    pub fn enforce_saved_state(&self) -> AppResult<Vec<String>> {
        let active_tips = self.state.enabled_tip_ids();
        if active_tips.is_empty() {
            return Err(AppError::EmptyLayoutList);
        }
        self.backend.apply_enabled_order(&active_tips)?;
        Ok(active_tips)
    }

    fn apply_enabled_index(
        &mut self,
        idx: usize,
        target_enabled: bool,
    ) -> AppResult<LayoutProfile> {
        let current_active_idx = self.active_profile_index();
        if let Some(plan) = self.plan_set_enabled_index(idx, target_enabled, current_active_idx)? {
            self.execute_plan_or_revert(plan)?;
        }
        Ok(self.state.profiles[idx].clone())
    }

    /// Single DRY planner shared by synchronous CLI (`set_enabled`/`toggle`) and optimistic GUI
    /// (`plan_toggle`). Mutates `self.state.profiles[idx].enabled` and returns `Ok(None)` if no-op.
    fn plan_set_enabled_index(
        &mut self,
        idx: usize,
        target_enabled: bool,
        current_active_idx: Option<usize>,
    ) -> AppResult<Option<ApplyPlan>> {
        if self.state.profiles[idx].enabled == target_enabled {
            return Ok(None);
        }

        if !target_enabled && self.enabled_count() <= 1 {
            return Err(AppError::CannotDisableLastLayout);
        }

        let prev_profiles = self.state.profiles.clone();
        let prev_active_tips = self.state.enabled_tip_ids();

        let disabling_active = current_active_idx == Some(idx) && !target_enabled;
        let desired_active_profile = match current_active_idx {
            Some(_) if disabling_active => self
                .state
                .profiles
                .iter()
                .enumerate()
                .find(|(i, p)| *i != idx && p.enabled)
                .map(|(_, p)| p.clone()),
            Some(cur_idx) => self.state.profiles.get(cur_idx).cloned(),
            None => None,
        };

        let switch_away_before = disabling_active
            .then(|| desired_active_profile.clone())
            .flatten();

        self.state.profiles[idx].enabled = target_enabled;
        let new_active_tips = self.state.enabled_tip_ids();

        Ok(Some(ApplyPlan {
            prev_profiles,
            prev_active_tips,
            new_active_tips,
            switch_away_before,
            restore_active_after: desired_active_profile,
            new_state: self.state.clone(),
        }))
    }

    fn execute_plan_or_revert(&mut self, plan: ApplyPlan) -> AppResult<()> {
        if let Err(err) = plan.execute(&self.backend, &self.repo) {
            self.revert_plan(plan.prev_profiles);
            return Err(err);
        }
        Ok(())
    }

    /// Updates tracked profiles, pruning externally removed active layouts while ALWAYS retaining
    /// intentionally disabled profiles (Windows removes their User Profile entry until re-enabled).
    fn reconcile_existing_profiles(
        &mut self,
        live_profiles: &[LayoutProfile],
        report: &mut SyncReport,
    ) -> bool {
        let mut changed = false;
        let mut retained = Vec::with_capacity(self.state.profiles.len());

        for mut existing in std::mem::take(&mut self.state.profiles) {
            if let Some(live) = live_profiles
                .iter()
                .find(|lp| lp.tip_id.eq_ignore_ascii_case(&existing.tip_id))
            {
                existing.refresh_metadata_from(live);
                if !existing.enabled {
                    existing.enabled = true;
                    changed = true;
                    report.reappeared_externally.push(existing.tip_id.clone());
                }
                retained.push(existing);
            } else if existing.enabled {
                changed = true;
                report.removed_externally.push(existing.tip_id.clone());
            } else {
                retained.push(existing);
            }
        }

        self.state.profiles = retained;
        changed
    }

    /// Discovers brand-new layouts/languages added externally in Windows Settings.
    fn append_new_live_profiles(
        &mut self,
        live_profiles: Vec<LayoutProfile>,
        report: &mut SyncReport,
    ) -> bool {
        let mut changed = false;

        for live in live_profiles {
            if !self
                .state
                .profiles
                .iter()
                .any(|p| p.tip_id.eq_ignore_ascii_case(&live.tip_id))
            {
                let id = live.tip_id.clone();
                if !self
                    .state
                    .original_snapshot
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case(&id))
                {
                    self.state.original_snapshot.push(id.clone());
                }
                self.state.profiles.push(live);
                report.added.push(id);
                changed = true;
            }
        }

        changed
    }

    fn resolve_selector(&self, selector: &str) -> AppResult<usize> {
        let trimmed = selector.trim();
        if let Ok(one_based) = trimmed.parse::<usize>() {
            if (1..=self.state.profiles.len()).contains(&one_based) {
                return Ok(one_based - 1);
            }
            return Err(AppError::IndexOutOfRange {
                index: one_based,
                max: self.state.profiles.len(),
            });
        }

        if let Some(pos) = self
            .state
            .profiles
            .iter()
            .position(|p| p.tip_id.eq_ignore_ascii_case(trimmed))
        {
            return Ok(pos);
        }

        if let Some(pos) = self.state.profiles.iter().position(|p| {
            p.bcp47.eq_ignore_ascii_case(trimmed) || p.short_badge.eq_ignore_ascii_case(trimmed)
        }) {
            return Ok(pos);
        }

        Err(AppError::ProfileNotFound(trimmed.to_string()))
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::cell::RefCell;

    use crate::core::error::{AppError, AppResult};
    use crate::core::model::{AppState, LayoutProfile, ProfileKind};
    use crate::core::ports::{InputBackend, StateRepository};

    pub struct FakeBackend {
        pub catalog: RefCell<Vec<LayoutProfile>>,
        pub enabled_order: RefCell<Vec<String>>,
        pub active_tip: RefCell<Option<String>>,
        pub fail_next_apply: RefCell<bool>,
        pub activation_log: RefCell<Vec<String>>,
    }

    impl FakeBackend {
        pub fn with_profiles(profiles: Vec<LayoutProfile>, active: &str) -> Self {
            let order = profiles.iter().map(|p| p.tip_id.clone()).collect();
            Self {
                catalog: RefCell::new(profiles),
                enabled_order: RefCell::new(order),
                active_tip: RefCell::new(Some(active.to_string())),
                fail_next_apply: RefCell::new(false),
                activation_log: RefCell::new(Vec::new()),
            }
        }

        pub fn enabled_order(&self) -> Vec<String> {
            self.enabled_order.borrow().clone()
        }
    }

    impl InputBackend for FakeBackend {
        fn list_enabled(&self) -> AppResult<Vec<LayoutProfile>> {
            let cat = self.catalog.borrow();
            let out = self
                .enabled_order
                .borrow()
                .iter()
                .filter_map(|id| cat.iter().find(|p| p.tip_id == *id).cloned())
                .collect();
            Ok(out)
        }

        fn apply_enabled_order(&self, tip_ids: &[String]) -> AppResult<()> {
            if *self.fail_next_apply.borrow() {
                *self.fail_next_apply.borrow_mut() = false;
                return Err(AppError::Backend(
                    "simulated InstallLayoutOrTip failure".into(),
                ));
            }
            if tip_ids.is_empty() {
                return Err(AppError::EmptyLayoutList);
            }
            *self.enabled_order.borrow_mut() = tip_ids.to_vec();
            Ok(())
        }

        fn active_tip_id(&self, _candidates: &[LayoutProfile]) -> Option<String> {
            self.active_tip.borrow().clone()
        }

        fn activate_profile(&self, profile: &LayoutProfile) -> AppResult<()> {
            *self.active_tip.borrow_mut() = Some(profile.tip_id.clone());
            self.activation_log
                .borrow_mut()
                .push(profile.tip_id.clone());
            Ok(())
        }
    }

    pub struct InMemoryRepo {
        pub saved: RefCell<Option<AppState>>,
        pub fail_next_save: RefCell<bool>,
    }

    impl InMemoryRepo {
        pub fn empty() -> Self {
            Self {
                saved: RefCell::new(None),
                fail_next_save: RefCell::new(false),
            }
        }
    }

    impl StateRepository for InMemoryRepo {
        fn load(&self) -> AppResult<Option<AppState>> {
            Ok(self.saved.borrow().clone())
        }

        fn save(&self, state: &AppState) -> AppResult<()> {
            if *self.fail_next_save.borrow() {
                *self.fail_next_save.borrow_mut() = false;
                return Err(AppError::Storage("simulated storage failure".into()));
            }
            *self.saved.borrow_mut() = Some(state.clone());
            Ok(())
        }

        fn state_location(&self) -> String {
            "<memory>".into()
        }

        fn recovery_location(&self) -> Option<String> {
            None
        }
    }

    pub fn sample_profile(
        tip: &str,
        langid: u16,
        bcp47: &str,
        badge: &str,
        lang: &str,
        layout: &str,
    ) -> LayoutProfile {
        LayoutProfile {
            tip_id: tip.into(),
            kind: ProfileKind::KeyboardLayout,
            langid,
            clsid: None,
            guid_profile: None,
            bcp47: bcp47.into(),
            short_badge: badge.into(),
            language_name: lang.into(),
            layout_name: layout.into(),
            enabled: true,
        }
    }

    pub fn sample_profiles() -> Vec<LayoutProfile> {
        vec![
            sample_profile(
                "0409:00000409",
                0x0409,
                "en-US",
                "ENG",
                "English (United States)",
                "US",
            ),
            sample_profile("0419:00000419", 0x0419, "ru", "РУС", "Russian", "Russian"),
            sample_profile(
                "0422:00020422",
                0x0422,
                "uk",
                "УКР",
                "Ukrainian",
                "Ukrainian (Enhanced)",
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{FakeBackend, InMemoryRepo, sample_profile, sample_profiles};
    use super::*;

    #[test]
    fn guards_against_disabling_last_remaining_layout() {
        let backend = FakeBackend::with_profiles(sample_profiles(), "0409:00000409");
        let mut mgr = LayoutManager::open(backend, InMemoryRepo::empty()).unwrap();

        mgr.set_enabled("3", false).unwrap();
        mgr.set_enabled("2", false).unwrap();

        let err = mgr.set_enabled("1", false).unwrap_err();
        assert_eq!(err, AppError::CannotDisableLastLayout);
        assert_eq!(mgr.enabled_count(), 1);
        assert_eq!(mgr.backend().enabled_order(), vec!["0409:00000409"]);
    }

    #[test]
    fn preserves_exact_original_order_when_reenabling_out_of_order() {
        let backend = FakeBackend::with_profiles(sample_profiles(), "0409:00000409");
        let mut mgr = LayoutManager::open(backend, InMemoryRepo::empty()).unwrap();

        mgr.set_enabled("3", false).unwrap();
        mgr.set_enabled("2", false).unwrap();
        // Re-enable #3 BEFORE #2
        mgr.set_enabled("3", true).unwrap();
        assert_eq!(
            mgr.backend().enabled_order(),
            vec!["0409:00000409", "0422:00020422"]
        );

        // Re-enable #2 -> MUST return to index 2, NOT end of list
        mgr.set_enabled("2", true).unwrap();
        assert_eq!(
            mgr.backend().enabled_order(),
            vec!["0409:00000409", "0419:00000419", "0422:00020422"]
        );
    }

    #[test]
    fn switches_away_before_disabling_currently_active_layout() {
        let backend = FakeBackend::with_profiles(sample_profiles(), "0419:00000419");
        let mut mgr = LayoutManager::open(backend, InMemoryRepo::empty()).unwrap();

        mgr.set_enabled("2", false).unwrap();
        assert_eq!(mgr.active_profile_index(), Some(0));
        assert!(
            mgr.backend()
                .activation_log
                .borrow()
                .iter()
                .any(|id| id == "0409:00000409")
        );
    }

    #[test]
    fn rolls_back_state_and_os_list_when_backend_apply_fails() {
        let backend = FakeBackend::with_profiles(sample_profiles(), "0409:00000409");
        let mut mgr = LayoutManager::open(backend, InMemoryRepo::empty()).unwrap();

        *mgr.backend().fail_next_apply.borrow_mut() = true;
        let err = mgr.set_enabled("3", false).unwrap_err();
        assert!(matches!(err, AppError::ApplyFailedRolledBack(_)));
        assert!(mgr.state().profiles[2].enabled);
        assert_eq!(mgr.enabled_count(), 3);
        assert_eq!(
            mgr.backend().enabled_order(),
            vec!["0409:00000409", "0419:00000419", "0422:00020422"]
        );
    }

    #[test]
    fn rolls_back_state_and_os_list_when_storage_save_fails() {
        let backend = FakeBackend::with_profiles(sample_profiles(), "0409:00000409");
        let repo = InMemoryRepo::empty();
        let mut mgr = LayoutManager::open(backend, repo).unwrap();

        *mgr.repo.fail_next_save.borrow_mut() = true;
        let err = mgr.set_enabled("3", false).unwrap_err();
        assert!(matches!(err, AppError::ApplyFailedRolledBack(_)));
        assert!(mgr.state().profiles[2].enabled);
        assert_eq!(
            mgr.backend().enabled_order(),
            vec!["0409:00000409", "0419:00000419", "0422:00020422"]
        );
    }

    #[test]
    fn retains_disabled_profiles_and_discovers_externally_added_layouts() {
        let backend = FakeBackend::with_profiles(sample_profiles(), "0409:00000409");
        let mut mgr = LayoutManager::open(backend, InMemoryRepo::empty()).unwrap();

        mgr.set_enabled("3", false).unwrap();

        // Simulate user adding German in Windows Settings.
        let de = sample_profile(
            "0407:00000407",
            0x0407,
            "de-DE",
            "DEU",
            "German (Germany)",
            "German",
        );
        mgr.backend().catalog.borrow_mut().push(de);
        mgr.backend()
            .enabled_order
            .borrow_mut()
            .push("0407:00000407".into());

        let report = mgr.sync_with_system().unwrap();
        assert_eq!(report.added, vec!["0407:00000407"]);
        assert!(report.removed_externally.is_empty());
        assert_eq!(mgr.state().profiles.len(), 4);
        assert!(!mgr.state().profiles[2].enabled);
        assert!(mgr.state().profiles[3].enabled);
    }

    #[test]
    fn detects_externally_removed_and_reappeared_layouts() {
        let backend = FakeBackend::with_profiles(sample_profiles(), "0409:00000409");
        let mut mgr = LayoutManager::open(backend, InMemoryRepo::empty()).unwrap();

        mgr.set_enabled("3", false).unwrap();
        // Externally remove Russian (#2) and externally re-enable Ukrainian (#3).
        *mgr.backend().enabled_order.borrow_mut() =
            vec!["0409:00000409".into(), "0422:00020422".into()];

        let report = mgr.sync_with_system().unwrap();
        assert_eq!(report.removed_externally, vec!["0419:00000419"]);
        assert_eq!(report.reappeared_externally, vec!["0422:00020422"]);
        assert_eq!(mgr.state().profiles.len(), 2);
        assert!(mgr.state().profiles.iter().all(|p| p.enabled));
    }

    #[test]
    fn restore_all_reenables_every_layout_in_original_order() {
        let backend = FakeBackend::with_profiles(sample_profiles(), "0409:00000409");
        let mut mgr = LayoutManager::open(backend, InMemoryRepo::empty()).unwrap();

        mgr.set_enabled("2", false).unwrap();
        mgr.set_enabled("3", false).unwrap();
        let count = mgr.restore_all().unwrap();

        assert_eq!(count, 3);
        assert_eq!(mgr.enabled_count(), 3);
        assert_eq!(
            mgr.backend().enabled_order(),
            vec!["0409:00000409", "0419:00000419", "0422:00020422"]
        );
    }

    #[test]
    fn optimistic_plan_toggle_and_revert_preserve_invariants() {
        let backend = FakeBackend::with_profiles(sample_profiles(), "0419:00000419");
        let mut mgr = LayoutManager::open(backend, InMemoryRepo::empty()).unwrap();

        let (updated, plan) = mgr.plan_toggle("2", Some(1)).unwrap();
        assert!(!updated.enabled);
        assert_eq!(plan.new_active_tips, vec!["0409:00000409", "0422:00020422"]);
        assert_eq!(
            plan.switch_away_before.as_ref().map(|p| p.tip_id.as_str()),
            Some("0409:00000409")
        );

        // Simulate async worker rollback
        mgr.revert_plan(plan.prev_profiles);
        assert_eq!(mgr.enabled_count(), 3);
        assert!(mgr.state().profiles[1].enabled);
    }

    #[test]
    fn resolves_selectors_by_index_tip_bcp47_and_badge() {
        let backend = FakeBackend::with_profiles(sample_profiles(), "0409:00000409");
        let mut mgr = LayoutManager::open(backend, InMemoryRepo::empty()).unwrap();

        assert_eq!(mgr.toggle("uk").unwrap().tip_id, "0422:00020422");
        assert_eq!(mgr.toggle("УКР").unwrap().tip_id, "0422:00020422");
        assert_eq!(mgr.toggle("0419:00000419").unwrap().tip_id, "0419:00000419");
        assert!(matches!(
            mgr.toggle("99"),
            Err(AppError::IndexOutOfRange { index: 99, max: 3 })
        ));
        assert!(matches!(
            mgr.toggle("fr-FR"),
            Err(AppError::ProfileNotFound(_))
        ));
    }

    #[test]
    fn rejects_activating_disabled_profile() {
        let backend = FakeBackend::with_profiles(sample_profiles(), "0409:00000409");
        let mut mgr = LayoutManager::open(backend, InMemoryRepo::empty()).unwrap();

        mgr.set_enabled("3", false).unwrap();
        assert!(matches!(
            mgr.activate("3"),
            Err(AppError::ProfileDisabled(id)) if id == "0422:00020422"
        ));
    }

    #[test]
    fn plan_restore_all_returns_none_when_everything_enabled() {
        let backend = FakeBackend::with_profiles(sample_profiles(), "0409:00000409");
        let mut mgr = LayoutManager::open(backend, InMemoryRepo::empty()).unwrap();
        assert!(mgr.plan_restore_all(Some(0)).is_none());
    }
}
