# ToggleLang

[![CI](https://github.com/To4ilochka/ToggleLang/actions/workflows/ci.yml/badge.svg)](https://github.com/To4ilochka/ToggleLang/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/To4ilochka/ToggleLang)](https://github.com/To4ilochka/ToggleLang/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform: Windows 11](https://img.shields.io/badge/Platform-Windows%2011-0078D4.svg)](#requirements)
[![Rust 1.88+](https://img.shields.io/badge/Rust-1.88%2B-orange.svg)](#build-from-source)

**ToggleLang** is a lightweight, native Windows 11 system tray utility and CLI that lets you **temporarily hide and restore keyboard layouts and IMEs** from `Alt+Shift` / `Win+Space` — without uninstalling Windows language packs or losing your layout order.

---

## Why ToggleLang?

If you keep 3 or more languages installed on Windows (for example, English + Russian + Ukrainian, or a CJK IME), cycling through layouts with `Alt+Shift` or `Win+Space` quickly gets in the way when you only need two of them right now.

Windows Settings only lets you *remove* a language or keyboard layout entirely. **ToggleLang** gives you instant per-layout ON/OFF switches in a native Windows 11 flyout so you can mute extra layouts for a task and bring them back in their exact original slot with a single click.

---

## Features

- **Native Windows 11 Fluent 2 UI** — Direct2D + DirectWrite flyout with DWM Acrylic backdrop (and Mica/opaque fallback), rounded corners, automatic Dark/Light theme and Windows accent color sync, Per-Monitor V2 DPI scaling, and smooth VSync animations.
- **Exact Order Preservation & Rollback Safety** — Re-enabling hidden layouts restores their exact original order. ToggleLang prevents disabling your last remaining active layout, switches away cleanly if you hide the currently active layout, and automatically rolls back on apply/storage errors.
- **Keyboards & IMEs Supported** — Works with both classic keyboard layouts (`KBD`) and Text Services Framework input processors (`IME`) via Windows `input.dll` and TSF COM APIs.
- **Two Tray Icon Modes** — Choose between a minimalist Fluent vector icon or a dynamic 2-letter / CJK language keycap (`EN`, `RU`, `UK`, `日`) that adapts to your taskbar theme and DPI.
- **Live System Sync** — Watches Windows language registry changes so languages added or removed in Windows Settings stay in sync automatically.
- **Built-in Emergency Recovery** — Every state save atomically updates `%LOCALAPPDATA%\ToggleLang\restore-all.ps1`, a standalone PowerShell script that restores all tracked layouts even if you delete `togglelang.exe`.
- **Scriptable CLI** — Inspect, enable, disable, toggle, activate, restore, or watch layouts from the terminal or hotkeys/scripts.
- **Tiny & Portable** — Single standalone executable (~550 KB), zero runtime dependencies, no telemetry, no installer, and **no administrator privileges required**.

---

## Requirements

- **Windows 11** (64-bit / `x86_64`)
- No admin rights or external runtimes required

---

## Installation

### Download a prebuilt release (Recommended)

1. Go to the [Latest Release](https://github.com/To4ilochka/ToggleLang/releases/latest).
2. Download either:
   - `togglelang-vX.Y.Z-x86_64-windows.exe` — standalone executable, ready to run directly, **or**
   - `togglelang-vX.Y.Z-x86_64-windows.zip` — portable archive containing `togglelang.exe`, `README.md`, and `LICENSE`.
3. *(Optional)* Verify the download against `SHA256SUMS.txt` in PowerShell:
   ```powershell
   Get-FileHash .\togglelang-v0.1.0-x86_64-windows.exe -Algorithm SHA256
   ```
4. Place `togglelang.exe` in any folder you like (for example `C:\Tools\ToggleLang\`) and launch it.

> **Tip:** Right-click the tray icon and enable **Start with Windows** to keep ToggleLang running after reboot (uses per-user `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`). Move `togglelang.exe` to its permanent folder *before* enabling autostart.

### Build from source

Requires **Rust 1.88+** (Edition 2024) on Windows:

```powershell
git clone https://github.com/To4ilochka/ToggleLang.git
cd ToggleLang
cargo build --release
```

The optimized binary will be written to `target\release\togglelang.exe`.

---

## Usage

### System Tray & Flyout (GUI)

- **Run `togglelang.exe`** (no arguments) — starts in the system tray. Launching a second instance wakes the existing flyout instead of spawning a duplicate.
- **Left-click tray icon** — opens the Fluent 2 flyout:
  - Click a **row** to switch the active input layout immediately.
  - Click the **toggle switch** on the right of a row to temporarily hide or restore that layout.
  - Navigate with `↑` / `↓`, `Enter`, `Space`, and `Esc`.
  - Click **Restore all** in the footer whenever any layout is hidden.
- **Right-click tray icon** — access options:
  - *Open keyboard layouts*
  - *Show language code as tray icon* (switches between vector icon and `EN`/`RU`/`UK` keycap)
  - *Start with Windows*
  - *Windows language settings* (`ms-settings:regionlanguage`)
  - *Restore all layouts* / *Restore all layouts and exit* / *Exit*

### Command-Line Interface (CLI)

`togglelang.exe` attaches to the parent console automatically when invoked with CLI arguments:

```text
togglelang [COMMAND]
```

| Command | Aliases | Description |
| :--- | :--- | :--- |
| *(none)* | `--tray` | Run in the system tray |
| `--show` | | Run in the system tray and open the flyout immediately |
| `list` | `ls` | Show tracked layouts, order, and `ON`/`OFF` state |
| `disable <selector>` | `off` | Temporarily hide a layout from `Alt+Shift` / `Win+Space` |
| `enable <selector>` | `on` | Restore a hidden layout in its exact original position |
| `toggle <selector>` | | Flip a layout between `ON` and `OFF` |
| `activate <selector>` | `switch` | Switch the active input layout for the current session |
| `restore` | `--restore-all` | Re-enable all tracked layouts in original order |
| `enforce` | | Re-apply currently saved `ON`/`OFF` state to Windows |
| `inspect` | `diag` | Print low-level `input.dll`, TSF, and Registry diagnostics |
| `watch` | | Watch Windows language registry changes in real time |
| `help` | `-h`, `--help` | Show CLI help |

`<selector>` accepts a **1-based index** (`1`, `2`, `3`), **TIP ID** (`0409:00000409`), **BCP-47 tag** (`en-US`, `uk`, `ru`), or **short badge** (`ENG`, `УКР`).

#### Examples

```powershell
# List all tracked layouts
togglelang list

# Temporarily hide the 3rd layout or Ukrainian layout
togglelang disable 3
togglelang off uk

# Bring it back in its original order
togglelang enable uk

# Restore every tracked layout
togglelang restore
```

---

## State Storage & Emergency Recovery

ToggleLang stores per-user data under `%LOCALAPPDATA%\ToggleLang\`:

| File | Purpose |
| :--- | :--- |
| `state.json` | Atomically persisted layout list, original order snapshot, ON/OFF states, and tray icon preference |
| `restore-all.ps1` | Auto-generated standalone PowerShell recovery script |
| `crash.log` | Fatal error / panic diagnostics (only created if an unhandled error occurs in GUI mode) |

If you ever want to restore all layouts without running ToggleLang, execute:

```powershell
powershell -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\ToggleLang\restore-all.ps1"
```

---

## Architecture

```text
src/
├── main.rs       # Composition root, DPI awareness, panic hook, console attach
├── cli.rs        # CLI argument parser & command dispatcher
├── core/         # Pure domain logic (no Win32 dependencies, fully unit-tested)
│   ├── model.rs  # LayoutProfile, AppState, TrayIconMode, KLID/keycap parsing
│   ├── ports.rs  # InputBackend & StateRepository traits (DIP)
│   ├── plan.rs   # Coalescing apply plans & rollback transactions
│   └── manager.rs# LayoutManager: reconciliation, order preservation, invariants
├── platform/     # Windows 11 OS adapters (input.dll, TSF COM, Registry, DWM, watcher)
└── ui/           # Direct2D/DirectWrite Fluent 2 renderer, animations, flyout & tray
```

---

## Contributing

Contributions, bug reports, and feature ideas are very welcome! Please read [CONTRIBUTING.md](CONTRIBUTING.md) and our [Code of Conduct](CODE_OF_CONDUCT.md) before opening an issue or pull request.

## Security

Please report security issues privately — see [SECURITY.md](SECURITY.md) for instructions.

## License

Licensed under the [MIT License](LICENSE).
