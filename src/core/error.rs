use std::fmt;

pub type AppResult<T> = Result<T, AppError>;

/// Domain and infrastructure errors with explicit semantics for UI, CLI, and rollback handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppError {
    CannotDisableLastLayout,
    EmptyLayoutList,
    IndexOutOfRange { index: usize, max: usize },
    ProfileNotFound(String),
    ProfileDisabled(String),
    Backend(String),
    Storage(String),
    ApplyFailedRolledBack(String),
    InvalidUsage(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CannotDisableLastLayout => {
                write!(f, "Cannot disable the last remaining active layout.")
            }
            Self::EmptyLayoutList => {
                write!(f, "Refusing to apply an empty layout list.")
            }
            Self::IndexOutOfRange { index, max } => {
                write!(f, "Index {index} is out of range (1..={max}).")
            }
            Self::ProfileNotFound(selector) => {
                write!(
                    f,
                    "No layout matches '{selector}'. Run `togglelang list` to see available layouts."
                )
            }
            Self::ProfileDisabled(tip_id) => {
                write!(
                    f,
                    "Layout '{tip_id}' is currently disabled. Enable it before activating."
                )
            }
            Self::Backend(msg) => write!(f, "Windows input backend error: {msg}"),
            Self::Storage(msg) => write!(f, "State storage error: {msg}"),
            Self::ApplyFailedRolledBack(msg) => {
                write!(f, "Failed to apply layout change (rolled back): {msg}")
            }
            Self::InvalidUsage(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for AppError {}
