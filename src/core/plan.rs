use crate::core::error::{AppError, AppResult};
use crate::core::model::{AppState, LayoutProfile};
use crate::core::ports::{InputBackend, StateRepository};

/// Atomic transaction plan for applying a layout visibility change to the OS and durable storage.
///
/// Shared by both synchronous CLI operations and the asynchronous GUI apply worker so that
/// ordering, switch-away protection, and rollback semantics are guaranteed identical everywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyPlan {
    pub prev_profiles: Vec<LayoutProfile>,
    pub prev_active_tips: Vec<String>,
    pub new_active_tips: Vec<String>,
    pub switch_away_before: Option<LayoutProfile>,
    pub restore_active_after: Option<LayoutProfile>,
    pub new_state: AppState,
}

impl ApplyPlan {
    /// Coalesces an earlier queued plan with a newer superseding plan from a burst of rapid toggles.
    ///
    /// Preserves the original rollback baseline (`first.prev_*`) and retains any required
    /// `switch_away_before` protection from the earlier plan if the newer plan did not specify one.
    pub fn coalesce(first: Self, newer: Self) -> Self {
        let switch_away_before = newer.switch_away_before.or(first.switch_away_before);
        Self {
            prev_profiles: first.prev_profiles,
            prev_active_tips: first.prev_active_tips,
            new_active_tips: newer.new_active_tips,
            switch_away_before,
            restore_active_after: newer.restore_active_after,
            new_state: newer.new_state,
        }
    }

    /// Executes the transaction against `backend` and `repo`, rolling back the OS active set if
    /// either OS application or durable state persistence fails.
    pub fn execute<B: InputBackend, S: StateRepository>(
        &self,
        backend: &B,
        repo: &S,
    ) -> AppResult<()> {
        if let Some(ref next_active) = self.switch_away_before {
            let _ = backend.activate_profile(next_active);
        }

        if let Err(err) = backend.apply_enabled_order(&self.new_active_tips) {
            let _ = backend.apply_enabled_order(&self.prev_active_tips);
            return Err(AppError::ApplyFailedRolledBack(err.to_string()));
        }

        if let Err(err) = repo.save(&self.new_state) {
            let _ = backend.apply_enabled_order(&self.prev_active_tips);
            return Err(AppError::ApplyFailedRolledBack(err.to_string()));
        }

        if let Some(ref restore_active) = self.restore_active_after {
            let _ = backend.activate_profile(restore_active);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

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

    struct StubBackend {
        applied: RefCell<Vec<Vec<String>>>,
        activated: RefCell<Vec<String>>,
        fail_first_apply: RefCell<bool>,
    }

    impl StubBackend {
        fn new(fail_first_apply: bool) -> Self {
            Self {
                applied: RefCell::new(Vec::new()),
                activated: RefCell::new(Vec::new()),
                fail_first_apply: RefCell::new(fail_first_apply),
            }
        }
    }

    impl InputBackend for StubBackend {
        fn list_enabled(&self) -> AppResult<Vec<LayoutProfile>> {
            Ok(Vec::new())
        }

        fn apply_enabled_order(&self, tip_ids: &[String]) -> AppResult<()> {
            self.applied.borrow_mut().push(tip_ids.to_vec());
            if *self.fail_first_apply.borrow() {
                *self.fail_first_apply.borrow_mut() = false;
                return Err(AppError::Backend("apply failed".into()));
            }
            Ok(())
        }

        fn active_tip_id(&self, _candidates: &[LayoutProfile]) -> Option<String> {
            None
        }

        fn activate_profile(&self, profile: &LayoutProfile) -> AppResult<()> {
            self.activated.borrow_mut().push(profile.tip_id.clone());
            Ok(())
        }
    }

    struct FailingRepo;

    impl StateRepository for FailingRepo {
        fn load(&self) -> AppResult<Option<AppState>> {
            Ok(None)
        }

        fn save(&self, _state: &AppState) -> AppResult<()> {
            Err(AppError::Storage("disk full".into()))
        }

        fn state_location(&self) -> String {
            "<failing>".into()
        }

        fn recovery_location(&self) -> Option<String> {
            None
        }
    }

    #[test]
    fn coalesce_preserves_initial_rollback_baseline_and_switch_away() {
        let en = make_profile("0409:00000409", true);
        let ru = make_profile("0419:00000419", true);
        let uk = make_profile("0422:00020422", true);

        let first = ApplyPlan {
            prev_profiles: vec![en.clone(), ru.clone(), uk.clone()],
            prev_active_tips: vec![en.tip_id.clone(), ru.tip_id.clone(), uk.tip_id.clone()],
            new_active_tips: vec![en.tip_id.clone(), uk.tip_id.clone()],
            switch_away_before: Some(en.clone()),
            restore_active_after: Some(en.clone()),
            new_state: AppState::default(),
        };
        let second = ApplyPlan {
            prev_profiles: vec![en.clone(), make_profile("0419:00000419", false), uk.clone()],
            prev_active_tips: vec![en.tip_id.clone(), uk.tip_id.clone()],
            new_active_tips: vec![en.tip_id.clone()],
            switch_away_before: None,
            restore_active_after: Some(en.clone()),
            new_state: AppState {
                version: 1,
                ..AppState::default()
            },
        };

        let merged = ApplyPlan::coalesce(first, second);
        assert_eq!(merged.prev_active_tips.len(), 3);
        assert_eq!(merged.new_active_tips, vec!["0409:00000409"]);
        assert_eq!(
            merged
                .switch_away_before
                .as_ref()
                .map(|p| p.tip_id.as_str()),
            Some("0409:00000409")
        );
    }

    #[test]
    fn rolls_back_os_state_when_repository_save_fails() {
        let backend = StubBackend::new(false);
        let en = make_profile("0409:00000409", true);
        let plan = ApplyPlan {
            prev_profiles: vec![en.clone()],
            prev_active_tips: vec!["0409:00000409".into(), "0419:00000419".into()],
            new_active_tips: vec!["0409:00000409".into()],
            switch_away_before: Some(en.clone()),
            restore_active_after: Some(en),
            new_state: AppState::default(),
        };

        let err = plan.execute(&backend, &FailingRepo).unwrap_err();
        assert!(matches!(err, AppError::ApplyFailedRolledBack(_)));
        assert_eq!(
            *backend.applied.borrow(),
            vec![
                vec!["0409:00000409".to_string()],
                vec!["0409:00000409".to_string(), "0419:00000419".to_string()],
            ]
        );
    }
}
