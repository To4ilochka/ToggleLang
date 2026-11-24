use std::io::{self, Write};

use crate::core::error::{AppError, AppResult};
use crate::core::manager::LayoutManager;
use crate::core::model::{LayoutProfile, ProfileKind};
use crate::core::ports::{InputBackend, StateRepository};
use crate::platform::SystemLayoutManager;
use crate::platform::registry::read_diagnostics;
use crate::platform::watcher::watch_language_registry;

/// Parsed CLI command (Command pattern: decouples argument parsing from execution).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Tray { show_immediately: bool },
    Help,
    List,
    Disable(String),
    Enable(String),
    Toggle(String),
    Activate(String),
    Restore,
    Enforce,
    Inspect,
    Watch,
}

impl Command {
    pub fn parse(args: &[String]) -> AppResult<Self> {
        let Some(cmd) = args.first().map(String::as_str) else {
            return Ok(Self::Tray {
                show_immediately: false,
            });
        };
        match cmd {
            "--tray" => Ok(Self::Tray {
                show_immediately: false,
            }),
            "--show" => Ok(Self::Tray {
                show_immediately: true,
            }),
            "help" | "-h" | "--help" => Ok(Self::Help),
            "list" | "ls" => Ok(Self::List),
            "disable" | "off" => require_selector(args, "disable").map(Self::Disable),
            "enable" | "on" => require_selector(args, "enable").map(Self::Enable),
            "toggle" => require_selector(args, "toggle").map(Self::Toggle),
            "activate" | "switch" => require_selector(args, "activate").map(Self::Activate),
            "restore" | "--restore-all" => Ok(Self::Restore),
            "enforce" => Ok(Self::Enforce),
            "inspect" | "diag" => Ok(Self::Inspect),
            "watch" => Ok(Self::Watch),
            other => Err(AppError::InvalidUsage(format!(
                "Unknown command '{other}'. Run `togglelang --help` for usage."
            ))),
        }
    }

    pub fn is_gui(&self) -> bool {
        matches!(self, Self::Tray { .. })
    }
}

pub fn execute(
    command: Command,
    open_manager: impl FnOnce() -> AppResult<SystemLayoutManager>,
) -> AppResult<()> {
    let mut stdout = io::stdout().lock();
    match command {
        Command::Tray { show_immediately } => {
            drop(stdout);
            let mgr = open_manager()?;
            crate::ui::app::run_tray_app(mgr, show_immediately)
        }
        Command::Help => {
            write_help(&mut stdout);
            Ok(())
        }
        Command::Inspect => {
            let mgr = open_manager()?;
            write_diagnostics(&mgr, &mut stdout)
        }
        Command::Watch => {
            let mut mgr = open_manager()?;
            let _ = writeln!(
                stdout,
                "Watching Windows language/layout registry changes (Ctrl+C to stop)..."
            );
            write_list(&mgr, &mut stdout);
            drop(stdout);
            watch_language_registry(|reason| {
                let mut out = io::stdout().lock();
                let _ = writeln!(out, "\n[watch] Change detected: {reason}");
                match mgr.sync_with_system() {
                    Ok(report) => {
                        if !report.added.is_empty() {
                            let _ = writeln!(out, "  + Added externally: {:?}", report.added);
                        }
                        if !report.removed_externally.is_empty() {
                            let _ = writeln!(
                                out,
                                "  - Removed externally: {:?}",
                                report.removed_externally
                            );
                        }
                        if !report.reappeared_externally.is_empty() {
                            let _ = writeln!(
                                out,
                                "  ! Re-enabled externally: {:?}",
                                report.reappeared_externally
                            );
                        }
                        write_list(&mgr, &mut out);
                    }
                    Err(e) => eprintln!("  [watch] sync error: {e}"),
                }
            })
        }
        domain_cmd => {
            let mut mgr = open_manager()?;
            execute_domain_command(&domain_cmd, &mut mgr, &mut stdout)
        }
    }
}

/// Port-abstracted domain command execution (DIP: testable against any `InputBackend`/`StateRepository`).
pub fn execute_domain_command<B: InputBackend, S: StateRepository>(
    command: &Command,
    mgr: &mut LayoutManager<B, S>,
    out: &mut impl Write,
) -> AppResult<()> {
    match command {
        Command::List => {
            write_list(mgr, out);
            Ok(())
        }
        Command::Disable(selector) => {
            let updated = mgr.set_enabled(selector, false)?;
            write_profile_action("Disabled", &updated, out);
            write_list(mgr, out);
            Ok(())
        }
        Command::Enable(selector) => {
            let updated = mgr.set_enabled(selector, true)?;
            write_profile_action("Enabled", &updated, out);
            write_list(mgr, out);
            Ok(())
        }
        Command::Toggle(selector) => {
            let updated = mgr.toggle(selector)?;
            let verb = if updated.enabled {
                "Enabled"
            } else {
                "Disabled"
            };
            write_profile_action(verb, &updated, out);
            write_list(mgr, out);
            Ok(())
        }
        Command::Activate(selector) => {
            let activated = mgr.activate(selector)?;
            write_profile_action("Activated", &activated, out);
            Ok(())
        }
        Command::Restore => {
            let count = mgr.restore_all()?;
            let _ = writeln!(out, "Restored all {count} layout(s) in original order.");
            write_list(mgr, out);
            Ok(())
        }
        Command::Enforce => {
            let tips = mgr.enforce_saved_state()?;
            let _ = writeln!(
                out,
                "Enforced saved state ({} active): {}",
                tips.len(),
                tips.join(";")
            );
            write_list(mgr, out);
            Ok(())
        }
        Command::Help | Command::Tray { .. } | Command::Inspect | Command::Watch => {
            Err(AppError::InvalidUsage(
                "Non-domain command cannot be dispatched to execute_domain_command".into(),
            ))
        }
    }
}

fn require_selector(args: &[String], subcommand: &str) -> AppResult<String> {
    args.get(1).cloned().ok_or_else(|| {
        AppError::InvalidUsage(format!(
            "Usage: togglelang {subcommand} <index|tip_id|bcp47>"
        ))
    })
}

fn write_profile_action(verb: &str, profile: &LayoutProfile, out: &mut impl Write) {
    let _ = writeln!(
        out,
        "{verb} [{}] {} — {} ({})",
        profile.short_badge, profile.language_name, profile.layout_name, profile.tip_id
    );
}

fn write_list<B: InputBackend, S: StateRepository>(
    mgr: &LayoutManager<B, S>,
    out: &mut impl Write,
) {
    let active_idx = mgr.active_profile_index();
    let _ = writeln!(out, "\nTracked keyboard layouts & input methods:");
    for (i, p) in mgr.state().profiles.iter().enumerate() {
        let marker = if active_idx == Some(i) { "*" } else { " " };
        let status = if p.enabled { "ON " } else { "OFF" };
        let kind = match p.kind {
            ProfileKind::KeyboardLayout => "KBD",
            ProfileKind::InputProcessor => "IME",
        };
        let _ = writeln!(
            out,
            " {marker} [{idx}] [{status}] {badge:<4} {lang} — {layout}  ({kind} | {bcp} | {tip})",
            idx = i + 1,
            badge = p.short_badge,
            lang = p.language_name,
            layout = p.layout_name,
            bcp = p.bcp47,
            tip = p.tip_id,
        );
    }
    let _ = writeln!(
        out,
        "\nActive in Alt+Shift / Win+Space: {}/{} | State: {}",
        mgr.enabled_count(),
        mgr.state().profiles.len(),
        mgr.state_location(),
    );
    if let Some(rec) = mgr.recovery_location() {
        let _ = writeln!(out, "Emergency recovery script: {rec}");
    }
}

fn write_diagnostics(mgr: &SystemLayoutManager, out: &mut impl Write) -> AppResult<()> {
    let _ = writeln!(out, "=== EnumEnabledLayoutOrTip (input.dll) ===");
    for (i, raw) in mgr.backend().raw_enabled()?.iter().enumerate() {
        let _ = writeln!(
            out,
            "  [{idx}] tip={tip} | type={ptype} | langid=0x{lang:04X} | subst=0x{sub:08X} | flags=0x{flags:04X}{def}",
            idx = i + 1,
            tip = raw.tip_id,
            ptype = raw.profile_type,
            lang = raw.langid,
            sub = raw.substitute_layout,
            flags = raw.flags,
            def = if raw.is_default() { " (DEFAULT)" } else { "" },
        );
    }

    let _ = writeln!(out, "\n=== TSF GetActiveProfile (msctf.dll) ===");
    match mgr.backend().active_tsf_profile() {
        Ok(a) => {
            let _ = writeln!(
                out,
                "  type={} | langid=0x{:04X} | hkl=0x{:08X}",
                a.profile_type, a.langid, a.hkl as u32
            );
        }
        Err(e) => {
            let _ = writeln!(out, "  error: {e}");
        }
    }

    let _ = writeln!(out, "\n=== Windows Registry Layers ===");
    let diag = read_diagnostics()?;
    let _ = writeln!(
        out,
        "  User Profile\\Languages: {:?}",
        diag.user_profile.languages
    );
    for lang in &diag.user_profile.languages {
        let tips = diag
            .user_profile
            .bcp47_tips
            .get(lang)
            .cloned()
            .unwrap_or_default();
        let _ = writeln!(out, "    User Profile\\{lang}: {tips:?}");
    }
    let _ = writeln!(out, "  Keyboard Layout\\Preload: {:?}", diag.preload);
    let _ = writeln!(
        out,
        "  Keyboard Layout\\Substitutes: {:?}",
        diag.substitutes
    );
    let _ = writeln!(out, "  CTF\\SortOrder\\Language: {:?}", diag.ctf_sort_order);

    write_list(mgr, out);
    Ok(())
}

fn write_help(out: &mut impl Write) {
    let _ = write!(
        out,
        r#"ToggleLang — Lightweight Windows 11 Keyboard Layout Manager

USAGE:
  togglelang [COMMAND]

GUI MODES:
  (no arguments)           Run in the system tray (default)
  --show                   Run in the system tray and open the flyout immediately

CLI COMMANDS:
  list                     Show tracked layouts, order, and ON/OFF state
  disable <idx|tip|bcp47>  Temporarily hide a layout from Alt+Shift and Win+Space
  enable  <idx|tip|bcp47>  Restore a hidden layout in its exact original position
  toggle  <idx|tip|bcp47>  Flip a layout between ON and OFF
  activate <idx|tip|bcp47> Switch active input layout for the current session
  restore | --restore-all  Re-enable all tracked layouts in original order
  enforce                  Re-apply currently saved ON/OFF state to Windows
  inspect                  Print low-level input.dll, TSF, and Registry diagnostics
  watch                    Watch Windows Settings for added/removed languages in real time
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::manager::test_support::{FakeBackend, InMemoryRepo, sample_profiles};

    fn vec_args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn parses_valid_commands_and_aliases() {
        assert_eq!(
            Command::parse(&vec_args(&[])).unwrap(),
            Command::Tray {
                show_immediately: false
            }
        );
        assert_eq!(
            Command::parse(&vec_args(&["--show"])).unwrap(),
            Command::Tray {
                show_immediately: true
            }
        );
        assert_eq!(Command::parse(&vec_args(&["ls"])).unwrap(), Command::List);
        assert_eq!(
            Command::parse(&vec_args(&["off", "3"])).unwrap(),
            Command::Disable("3".into())
        );
        assert_eq!(
            Command::parse(&vec_args(&["on", "uk"])).unwrap(),
            Command::Enable("uk".into())
        );
        assert_eq!(
            Command::parse(&vec_args(&["--restore-all"])).unwrap(),
            Command::Restore
        );
    }

    #[test]
    fn rejects_missing_selector_and_unknown_command() {
        assert!(matches!(
            Command::parse(&vec_args(&["disable"])),
            Err(AppError::InvalidUsage(_))
        ));
        assert!(matches!(
            Command::parse(&vec_args(&["bogus"])),
            Err(AppError::InvalidUsage(_))
        ));
    }

    #[test]
    fn executes_domain_commands_through_ports_and_formats_output() {
        let backend = FakeBackend::with_profiles(sample_profiles(), "0409:00000409");
        let mut mgr = LayoutManager::open(backend, InMemoryRepo::empty()).unwrap();
        let mut buf = Vec::new();

        execute_domain_command(&Command::Disable("3".into()), &mut mgr, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("Disabled [УКР]"));
        assert!(out.contains("Active in Alt+Shift / Win+Space: 2/3"));

        let mut buf = Vec::new();
        execute_domain_command(&Command::Activate("2".into()), &mut mgr, &mut buf).unwrap();
        assert!(String::from_utf8(buf).unwrap().contains("Activated [РУС]"));

        let mut buf = Vec::new();
        execute_domain_command(&Command::Restore, &mut mgr, &mut buf).unwrap();
        assert!(String::from_utf8(buf).unwrap().contains("Restored all 3"));

        let mut buf = Vec::new();
        execute_domain_command(&Command::Enforce, &mut mgr, &mut buf).unwrap();
        assert!(
            String::from_utf8(buf)
                .unwrap()
                .contains("Enforced saved state (3 active)")
        );
    }
}
