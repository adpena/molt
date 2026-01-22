# Molt install (binary release)

This bundle includes the Molt CLI and (optionally) the `molt-worker` helper.
It bootstraps a local Python venv on first run and installs Molt into it.

## Requirements

- **Python 3.12+** available as `python3` (or `python` on Windows).
- **Rust toolchain** (`rustup` recommended) so Molt can build the runtime/backend.
- **C/C++ toolchain**:
  - macOS: Xcode Command Line Tools (`xcode-select --install`)
  - Linux: clang/llvm + build essentials
  - Windows: LLVM clang or set `CC` to a compatible compiler

## Install

### Package managers (recommended)

Homebrew (macOS/Linux):

```bash
brew tap adpena/molt
brew install molt
```

Optional minimal worker:

```bash
brew install molt-worker
```

Winget (Windows):

```powershell
winget install Adpena.Molt
```

If winget doesn't list `Adpena.Molt` yet, use Scoop or the script installer below.

Scoop (Windows):

```powershell
scoop bucket add adpena https://github.com/adpena/scoop-molt
scoop install molt
```

### Script install (binary bundle)

1. Put the `bin/` directory on your `PATH`.
2. Run `molt doctor` to verify toolchains.
3. Build and run:

```bash
molt build examples/hello.py
~/.molt/bin/hello_molt
```

## Verification checklist

Run after install:

macOS/Linux:

```bash
molt doctor --json
molt build examples/hello.py
```

Windows (PowerShell):

```powershell
molt doctor --json
molt build examples\\hello.py
```

Expected: JSON output, exit code 0. Compiled binary under `$MOLT_BIN` (defaults to
`~/.molt/bin` on Unix, `%USERPROFILE%\\.molt\\bin` on Windows).

Example JSON shape (values vary):

```json
{
  "schema_version": "1.0",
  "command": "doctor",
  "status": "ok",
  "data": {
    "checks": [
      {"name": "python", "ok": true, "detail": "3.12.x (requires >=3.12)"},
      {"name": "uv", "ok": true, "detail": "<path-to-uv>"},
      {"name": "cargo", "ok": true, "detail": "<path-to-cargo>"}
    ]
  },
  "warnings": [],
  "errors": []
}
```

## Optional environment overrides

- `MOLT_HOME`: override the data/build root (defaults to `~/.molt` unless the bundle is writable)
- `MOLT_VENV`: override the bootstrap venv path
- `MOLT_PROJECT_ROOT`: overrides project root resolution
