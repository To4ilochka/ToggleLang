pub mod active_input;
pub mod apply_worker;
pub mod backend;
pub mod input_api;
pub mod locale;
pub mod registry;
pub mod shell;
pub mod single_instance;
pub mod storage;
pub mod theme;
pub mod tsf;
pub mod watcher;
pub mod win_util;

use crate::core::manager::LayoutManager;
use crate::platform::backend::WindowsInputBackend;
use crate::platform::storage::FileStateRepository;

/// Production `LayoutManager` bound to the concrete Windows 11 backend and file repository.
pub type SystemLayoutManager = LayoutManager<WindowsInputBackend, FileStateRepository>;
