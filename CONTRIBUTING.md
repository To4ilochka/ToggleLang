# Contributing to ToggleLang

Thank you for your interest in contributing to **ToggleLang**! Bug reports, feature requests, documentation improvements, and pull requests are all welcome.

Please note that this project follows a [Code of Conduct](CODE_OF_CONDUCT.md). By participating, you agree to uphold it.

---

## Ways to Contribute

- **Report a bug:** Open a [Bug Report](https://github.com/To4ilochka/ToggleLang/issues/new?template=bug_report.yml). Including the output of `togglelang inspect` helps diagnose layout and IME issues quickly.
- **Suggest a feature:** Open a [Feature Request](https://github.com/To4ilochka/ToggleLang/issues/new?template=feature_request.yml) and describe the use case.
- **Submit a pull request:** Fix a bug, improve documentation, add tests, or implement an agreed feature. For large architectural or UX changes, please open an issue first to discuss the approach.

---

## Development Setup

### Prerequisites

- **Windows 11** (x86_64)
- **Rust 1.88+** (Edition 2024) — install via [rustup](https://rustup.rs/)
- Either the `x86_64-pc-windows-msvc` (Visual Studio C++ Build Tools) or `x86_64-pc-windows-gnu` toolchain

### Building & Running Locally

```powershell
git clone https://github.com/<your-username>/ToggleLang.git
cd ToggleLang

# Run unit tests
cargo test

# Run the tray app (opens flyout immediately)
cargo run -- --show

# Run CLI commands
cargo run -- list
cargo run -- inspect
```

---

## Code Quality & Architecture Guidelines

Before submitting a pull request, make sure all local checks pass (the same checks run automatically in GitHub Actions CI):

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

### Design Principles

1. **Keep `src/core/` platform-independent:** Domain logic (`LayoutManager`, `ApplyPlan`, `AppState`) must depend only on traits in `src/core/ports.rs` — never directly on Win32 APIs. This keeps domain invariants fast and deterministically unit-testable.
2. **Document every `unsafe` block:** Every `unsafe` Win32/FFI call must have a clear `// SAFETY:` comment explaining why the invariants hold.
3. **Prefer RAII guards for OS resources:** Wrap Win32 handles, DLL modules, DCs, bitmaps, and windows in RAII wrappers (see `src/platform/win_util.rs` and `src/ui/window.rs`) so resources cannot leak on error paths.
4. **Preserve layout safety invariants:** Never allow disabling the last enabled layout, always preserve original layout order, and ensure state/OS rollback works if saving or applying fails.
5. **Write tests for new behavior:** Add unit tests alongside your changes (`#[cfg(test)] mod tests`) whenever adding domain logic, CLI commands, layout math, or parsing.

---

## Pull Request Process

1. **Fork** the repository and create a branch from `main`:
   ```powershell
   git checkout -b feat/short-description
   ```
2. Keep commits focused and descriptive (e.g., `fix(tray): handle high-DPI icon refresh`, `feat(cli): add json output flag`).
3. Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.
4. Push your branch to your fork and open a **Pull Request** against `To4ilochka/ToggleLang:main`.
5. Fill out the PR template and describe how you tested your changes.
6. Wait for CI checks and maintainer review.

Thank you for helping make ToggleLang better!
