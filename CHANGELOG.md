# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1] - 2026-11-24

### Changed
- Upgraded Windows bindings from `windows 0.58` to `windows 0.62.2` and `windows-numerics 0.3` with stricter `Option`-typed Win32 handles and Direct2D numeric types.
- Updated GitHub Actions workflows to `actions/checkout@v7`.
- Documented `windows 0.62` `raw-dylib` toolchain requirements for `x86_64-pc-windows-gnu` in `CONTRIBUTING.md`.

## [0.1.0] - 2026-11-24

### Added
- Native Windows 11 system tray utility with Fluent 2 Acrylic flyout (Direct2D + DirectWrite, Dark/Light mode, accent color sync, Per-Monitor V2 DPI, VSync animations, keyboard navigation).
- Per-layout temporary hide/restore via `input.dll` & TSF COM APIs for both keyboard layouts and IMEs, preserving exact original layout order.
- Safety invariants: prevents disabling the last active layout, switches away cleanly when hiding the active layout, and rolls back automatically on errors.
- Two tray icon modes: minimalist Fluent vector icon and dynamic language keycap (`EN`, `RU`, `UK`, etc.).
- Live Windows language registry watcher to keep external Windows Settings changes in sync.
- Per-user autostart toggle (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`) and single-instance wake guard.
- Atomic state persistence (`%LOCALAPPDATA%\ToggleLang\state.json`) and auto-generated emergency recovery script (`restore-all.ps1`).
- Full CLI (`list`, `disable`, `enable`, `toggle`, `activate`, `restore`, `enforce`, `inspect`, `watch`).

[Unreleased]: https://github.com/To4ilochka/ToggleLang/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/To4ilochka/ToggleLang/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/To4ilochka/ToggleLang/releases/tag/v0.1.0
