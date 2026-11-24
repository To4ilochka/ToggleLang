use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::core::error::{AppError, AppResult};
use crate::core::model::LayoutProfile;
use crate::core::plan::ApplyPlan;
use crate::core::ports::{InputBackend, StateRepository};
use crate::platform::backend::WindowsInputBackend;
use crate::platform::storage::FileStateRepository;

pub const WM_APPLY_COMPLETE: u32 = WM_APP + 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    Succeeded,
    FailedRolledBack {
        prev_profiles: Vec<LayoutProfile>,
        error: String,
    },
}

/// Dedicated background worker that runs `ApplyPlan::execute` completely off the UI thread
/// so flyout animations stay locked at 60–240 Hz with 0 ms jank.
pub struct ApplyWorker {
    tx: Option<Sender<ApplyPlan>>,
    outcomes_rx: Receiver<ApplyOutcome>,
    handle: Option<JoinHandle<()>>,
}

impl ApplyWorker {
    /// Spawns the production Windows STA worker posting `WM_APPLY_COMPLETE` to `notify_hwnd`.
    pub fn spawn(notify_hwnd: HWND) -> AppResult<Self> {
        let raw_hwnd = notify_hwnd.0 as usize;
        Self::spawn_with(
            || {
                let backend = WindowsInputBackend::new()?;
                let repo = FileStateRepository::default_location();
                Ok((backend, repo))
            },
            move || {
                let hwnd = HWND(raw_hwnd as *mut core::ffi::c_void);
                // SAFETY: `PostMessageW` is thread-safe and ignored harmlessly if the window closed.
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APPLY_COMPLETE, WPARAM(0), LPARAM(0));
                }
            },
        )
    }

    /// Generic constructor depending strictly on `InputBackend` and `StateRepository` ports (DIP).
    pub fn spawn_with<B, S, F, N>(init_ports: F, notify: N) -> AppResult<Self>
    where
        B: InputBackend,
        S: StateRepository,
        F: FnOnce() -> AppResult<(B, S)> + Send + 'static,
        N: Fn() + Send + 'static,
    {
        let (plan_tx, plan_rx) = mpsc::channel::<ApplyPlan>();
        let (outcome_tx, outcome_rx) = mpsc::channel::<ApplyOutcome>();
        let (ready_tx, ready_rx) = mpsc::channel::<AppResult<()>>();

        let handle = thread::Builder::new()
            .name("togglelang-apply".into())
            .spawn(move || {
                let (backend, repo) = match init_ports() {
                    Ok(ports) => {
                        let _ = ready_tx.send(Ok(()));
                        ports
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                worker_loop(&backend, &repo, plan_rx, outcome_tx, notify);
            })
            .map_err(|e| AppError::Backend(format!("Failed to spawn apply worker thread: {e}")))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                tx: Some(plan_tx),
                outcomes_rx: outcome_rx,
                handle: Some(handle),
            }),
            Ok(Err(err)) => {
                let _ = handle.join();
                Err(err)
            }
            Err(_) => {
                let _ = handle.join();
                Err(AppError::Backend(
                    "Apply worker exited unexpectedly during initialization".into(),
                ))
            }
        }
    }

    /// Enqueues an `ApplyPlan` for immediate off-thread execution.
    pub fn submit(&self, plan: ApplyPlan) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(plan);
        }
    }

    /// Drains all completed apply outcomes posted by the worker thread without blocking.
    pub fn drain_outcomes(&self) -> Vec<ApplyOutcome> {
        self.outcomes_rx.try_iter().collect()
    }

    /// Closes the submission channel and waits for any in-flight plan to finish cleanly.
    /// Used on orderly shutdown (e.g. `RestoreAndExit`) to avoid racing the UI thread.
    pub fn shutdown(&mut self) {
        self.tx.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ApplyWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn worker_loop<B, S, N>(
    backend: &B,
    repo: &S,
    rx: Receiver<ApplyPlan>,
    outcomes: Sender<ApplyOutcome>,
    notify: N,
) where
    B: InputBackend,
    S: StateRepository,
    N: Fn(),
{
    while let Ok(mut merged) = rx.recv() {
        // Coalesce rapid successive toggles into a single atomic OS apply while preserving
        // the original rollback baseline and any switch-away protection.
        while let Ok(newer) = rx.try_recv() {
            merged = ApplyPlan::coalesce(merged, newer);
        }

        let outcome = match merged.execute(backend, repo) {
            Ok(()) => ApplyOutcome::Succeeded,
            Err(err) => ApplyOutcome::FailedRolledBack {
                prev_profiles: merged.prev_profiles,
                error: err.to_string(),
            },
        };

        let _ = outcomes.send(outcome);
        notify();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::core::model::{AppState, ProfileKind};

    fn profile(tip: &str, enabled: bool) -> LayoutProfile {
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

    struct PauseFirstApply {
        entered_tx: Sender<()>,
        release_rx: Mutex<Option<Receiver<()>>>,
    }

    struct SharedFakeBackend {
        applied: Arc<Mutex<Vec<Vec<String>>>>,
        activated: Arc<Mutex<Vec<String>>>,
        fail_first_apply: Arc<Mutex<bool>>,
        pause_first: Option<PauseFirstApply>,
    }

    impl InputBackend for SharedFakeBackend {
        fn list_enabled(&self) -> AppResult<Vec<LayoutProfile>> {
            Ok(Vec::new())
        }

        fn apply_enabled_order(&self, tip_ids: &[String]) -> AppResult<()> {
            if let Some(ref pause) = self.pause_first
                && let Some(rx) = pause.release_rx.lock().unwrap().take()
            {
                let _ = pause.entered_tx.send(());
                let _ = rx.recv();
            }
            self.applied.lock().unwrap().push(tip_ids.to_vec());
            let mut fail = self.fail_first_apply.lock().unwrap();
            if *fail {
                *fail = false;
                return Err(AppError::Backend("boom".into()));
            }
            Ok(())
        }

        fn active_tip_id(&self, _candidates: &[LayoutProfile]) -> Option<String> {
            None
        }

        fn activate_profile(&self, p: &LayoutProfile) -> AppResult<()> {
            self.activated.lock().unwrap().push(p.tip_id.clone());
            Ok(())
        }
    }

    struct SharedFakeRepo {
        saved: Arc<Mutex<Vec<AppState>>>,
        fail_save: bool,
    }

    impl StateRepository for SharedFakeRepo {
        fn load(&self) -> AppResult<Option<AppState>> {
            Ok(None)
        }

        fn save(&self, state: &AppState) -> AppResult<()> {
            if self.fail_save {
                return Err(AppError::Storage("read-only disk".into()));
            }
            self.saved.lock().unwrap().push(state.clone());
            Ok(())
        }

        fn state_location(&self) -> String {
            "<test>".into()
        }

        fn recovery_location(&self) -> Option<String> {
            None
        }
    }

    #[test]
    fn executes_plan_and_notifies_caller() {
        let applied = Arc::new(Mutex::new(Vec::new()));
        let activated = Arc::new(Mutex::new(Vec::new()));
        let saved = Arc::new(Mutex::new(Vec::new()));
        let notifications = Arc::new(AtomicUsize::new(0));

        let a_bg = Arc::clone(&applied);
        let act_bg = Arc::clone(&activated);
        let s_bg = Arc::clone(&saved);
        let n_bg = Arc::clone(&notifications);

        let mut worker = ApplyWorker::spawn_with(
            move || {
                Ok((
                    SharedFakeBackend {
                        applied: a_bg,
                        activated: act_bg,
                        fail_first_apply: Arc::new(Mutex::new(false)),
                        pause_first: None,
                    },
                    SharedFakeRepo {
                        saved: s_bg,
                        fail_save: false,
                    },
                ))
            },
            move || {
                n_bg.fetch_add(1, Ordering::SeqCst);
            },
        )
        .unwrap();

        let en = profile("0409:00000409", true);
        worker.submit(ApplyPlan {
            prev_profiles: vec![en.clone()],
            prev_active_tips: vec!["0409:00000409".into(), "0419:00000419".into()],
            new_active_tips: vec!["0409:00000409".into()],
            switch_away_before: Some(en.clone()),
            restore_active_after: Some(en),
            new_state: AppState::default(),
        });
        worker.shutdown();

        assert_eq!(worker.drain_outcomes(), vec![ApplyOutcome::Succeeded]);
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
        assert_eq!(*applied.lock().unwrap(), vec![vec!["0409:00000409"]]);
        assert_eq!(saved.lock().unwrap().len(), 1);
    }

    #[test]
    fn rolls_back_and_reports_failure_when_save_fails() {
        let applied = Arc::new(Mutex::new(Vec::new()));
        let a_bg = Arc::clone(&applied);

        let mut worker = ApplyWorker::spawn_with(
            move || {
                Ok((
                    SharedFakeBackend {
                        applied: a_bg,
                        activated: Arc::new(Mutex::new(Vec::new())),
                        fail_first_apply: Arc::new(Mutex::new(false)),
                        pause_first: None,
                    },
                    SharedFakeRepo {
                        saved: Arc::new(Mutex::new(Vec::new())),
                        fail_save: true,
                    },
                ))
            },
            || {},
        )
        .unwrap();

        let prev = vec![
            profile("0409:00000409", true),
            profile("0419:00000419", true),
        ];
        worker.submit(ApplyPlan {
            prev_profiles: prev.clone(),
            prev_active_tips: vec!["0409:00000409".into(), "0419:00000419".into()],
            new_active_tips: vec!["0409:00000409".into()],
            switch_away_before: None,
            restore_active_after: None,
            new_state: AppState::default(),
        });
        worker.shutdown();

        let outcomes = worker.drain_outcomes();
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            &outcomes[0],
            ApplyOutcome::FailedRolledBack { prev_profiles, .. } if *prev_profiles == prev
        ));
        // Verified: OS active list was rolled back to the original 2 layouts!
        assert_eq!(
            *applied.lock().unwrap(),
            vec![
                vec!["0409:00000409".to_string()],
                vec!["0409:00000409".to_string(), "0419:00000419".to_string()],
            ]
        );
    }

    #[test]
    fn coalesces_burst_of_queued_plans_while_preserving_rollback_baseline() {
        let applied = Arc::new(Mutex::new(Vec::new()));
        let activated = Arc::new(Mutex::new(Vec::new()));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let completed = Arc::new(AtomicUsize::new(0));

        let a_bg = Arc::clone(&applied);
        let act_bg = Arc::clone(&activated);
        let done_bg = Arc::clone(&completed);

        let mut worker = ApplyWorker::spawn_with(
            move || {
                Ok((
                    SharedFakeBackend {
                        applied: a_bg,
                        activated: act_bg,
                        fail_first_apply: Arc::new(Mutex::new(false)),
                        pause_first: Some(PauseFirstApply {
                            entered_tx,
                            release_rx: Mutex::new(Some(release_rx)),
                        }),
                    },
                    SharedFakeRepo {
                        saved: Arc::new(Mutex::new(Vec::new())),
                        fail_save: false,
                    },
                ))
            },
            move || {
                done_bg.fetch_add(1, Ordering::SeqCst);
            },
        )
        .unwrap();

        let en = profile("0409:00000409", true);
        // Plan 1 starts executing and deterministically signals entry before pausing.
        worker.submit(ApplyPlan {
            prev_profiles: vec![en.clone()],
            prev_active_tips: vec!["0409:00000409".into(), "0419:00000419".into()],
            new_active_tips: vec!["0409:00000409".into()],
            switch_away_before: None,
            restore_active_after: None,
            new_state: AppState::default(),
        });
        entered_rx.recv().unwrap();

        // Submit Plan 2 and Plan 3 while worker is blocked on Plan 1; they MUST coalesce.
        worker.submit(ApplyPlan {
            prev_profiles: vec![en.clone()],
            prev_active_tips: vec!["0409:00000409".into()],
            new_active_tips: vec!["0409:00000409".into(), "0419:00000419".into()],
            switch_away_before: Some(en.clone()),
            restore_active_after: None,
            new_state: AppState::default(),
        });
        worker.submit(ApplyPlan {
            prev_profiles: vec![en.clone()],
            prev_active_tips: vec!["0409:00000409".into(), "0419:00000419".into()],
            new_active_tips: vec![
                "0409:00000409".into(),
                "0419:00000419".into(),
                "0422:00020422".into(),
            ],
            switch_away_before: None,
            restore_active_after: Some(en),
            new_state: AppState::default(),
        });

        // Unblock Plan 1 and let the worker coalesce & execute Plan (2+3).
        release_tx.send(()).unwrap();
        worker.shutdown();

        assert_eq!(completed.load(Ordering::SeqCst), 2);
        assert_eq!(
            *applied.lock().unwrap(),
            vec![
                vec!["0409:00000409".to_string()],
                vec![
                    "0409:00000409".to_string(),
                    "0419:00000419".to_string(),
                    "0422:00020422".to_string(),
                ],
            ]
        );
        // Switch-away from Plan 2 survived coalescing into Plan 3.
        assert_eq!(activated.lock().unwrap().len(), 2);
    }

    #[test]
    fn propagates_backend_initialization_failure() {
        let res = ApplyWorker::spawn_with::<SharedFakeBackend, SharedFakeRepo, _, _>(
            || Err(AppError::Backend("init failed".into())),
            || {},
        );
        assert!(matches!(res, Err(AppError::Backend(_))));
    }
}
