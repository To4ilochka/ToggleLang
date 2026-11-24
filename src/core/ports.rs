use crate::core::error::AppResult;
use crate::core::model::{AppState, LayoutProfile};

/// Port abstracting OS-level input profile enumeration, ordering, and session switching.
pub trait InputBackend {
    /// Returns all layouts/IMEs currently active in the OS switching cycle, in exact order.
    fn list_enabled(&self) -> AppResult<Vec<LayoutProfile>>;

    /// Atomically applies the exact ordered list of enabled TIP strings to the OS.
    fn apply_enabled_order(&self, tip_ids: &[String]) -> AppResult<()>;

    /// Resolves the canonical TIP ID of the currently active profile among `candidates`.
    fn active_tip_id(&self, candidates: &[LayoutProfile]) -> Option<String>;

    /// Switches the active input profile for the current user session.
    fn activate_profile(&self, profile: &LayoutProfile) -> AppResult<()>;
}

/// Port abstracting durable state persistence and emergency recovery metadata.
pub trait StateRepository {
    fn load(&self) -> AppResult<Option<AppState>>;
    fn save(&self, state: &AppState) -> AppResult<()>;
    fn state_location(&self) -> String;
    fn recovery_location(&self) -> Option<String>;
}
