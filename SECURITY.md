# Security Policy

## Supported Versions

Only the latest released version of ToggleLang receives security fixes.

| Version | Supported |
| :--- | :--- |
| Latest release (`0.1.x`) | Yes |
| Older versions | No |

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

Instead, please report them privately using [GitHub Security Advisories](https://github.com/To4ilochka/ToggleLang/security/advisories/new).

Please include as much of the following information as possible:

- A clear description of the vulnerability and potential impact
- Steps to reproduce or a minimal proof of concept
- Affected ToggleLang version and Windows build number
- Any suggested mitigation or fix (if you have one)

You can expect an initial acknowledgment within 7 days and status updates as the report is investigated and resolved.

## Security Design Notes

- ToggleLang runs strictly under the current user account and never requires administrator privileges.
- System DLLs (`input.dll`, `uxtheme.dll`) are loaded exclusively from `System32` via `LOAD_LIBRARY_SEARCH_SYSTEM32` to prevent DLL search-order hijacking.
- Local state (`state.json` and `restore-all.ps1`) is stored per-user under `%LOCALAPPDATA%\ToggleLang` using atomic file replacement.
- ToggleLang makes zero network connections and collects no telemetry.
- Official release binaries are built directly from source on GitHub Actions and published alongside `SHA256SUMS.txt`.
