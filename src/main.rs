#![cfg_attr(not(test), windows_subsystem = "windows")]

mod cli;
mod core;
mod platform;
mod ui;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::process::ExitCode;

use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

use crate::cli::Command;
use crate::core::error::AppResult;
use crate::core::manager::LayoutManager;
use crate::platform::SystemLayoutManager;
use crate::platform::backend::WindowsInputBackend;
use crate::platform::storage::FileStateRepository;
use crate::platform::win_util::app_data_dir;

fn main() -> ExitCode {
    install_panic_hook();
    enable_per_monitor_dpi();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = Command::parse(&args);

    let needs_console = match &parsed {
        Ok(cmd) => !cmd.is_gui(),
        Err(_) => true,
    };
    if needs_console {
        attach_parent_console();
    }

    match parsed.and_then(|cmd| cli::execute(cmd, open_system_manager)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            if needs_console {
                eprintln!("Error: {err}");
            } else {
                log_crash(&format!("Fatal error: {err}"));
            }
            ExitCode::FAILURE
        }
    }
}

/// Composition root: wires concrete Windows backend & file repository into the domain manager.
fn open_system_manager() -> AppResult<SystemLayoutManager> {
    let backend = WindowsInputBackend::new()?;
    let repo = FileStateRepository::default_location();
    LayoutManager::open(backend, repo)
}

fn enable_per_monitor_dpi() {
    // SAFETY: Must be called before creating any Win32 windows for crisp Per-Monitor V2 DPI scaling.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn attach_parent_console() {
    // SAFETY: Attaches stdout/stderr to parent console when CLI flags are passed to a GUI-subsystem exe.
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        log_crash(&format!("Panic: {info}"));
    }));
}

fn log_crash(message: &str) {
    let dir = app_data_dir();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("crash.log");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{message}");
    }
}
