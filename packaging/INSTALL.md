# Molt install (binary release)

This bundle includes the Molt CLI, a production-optimized compiler, matching
compiler/runtime sources, and the `molt-worker` helper.
The Molt toolchain may bootstrap local build dependencies on the development machine,
but binaries produced by `molt build` are expected to run on target machines without any
host Python installation or hidden CPython fallback.

## Requirements

- **Python 3.12+** available as `python3` (or `python` on Windows).
- **uv** manages the CLI's isolated dependencies directly from the bundled
  `uv.lock`, including its artifact hashes and Python/platform markers.
- **Rust toolchain** (`rustup` recommended) so Molt can build the selected runtime.
  The shipped compiler itself is already built; selecting a guest profile does
  not rebuild it.
- **C/C++ toolchain**:
  - macOS: Xcode Command Line Tools (`xcode-select --install`)
  - Linux: clang/llvm + build essentials
  - Windows: LLVM clang or set `CC` to a compatible compiler

Set `PYTHON` to a CPython executable path to select an interpreter explicitly;
the POSIX, Command Prompt and PowerShell launchers honor the same override.
The bootstrap checks the CPython 3.12+ minimum. Verified versions and target
support remain those listed in the release matrix.

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

`molt build app.py --profile dev` and `--profile release` use the same production
compiler. `--diagnostics` distinguishes its profile and content identity from
the program/runtime profiles. Native/WASM feature availability remains governed
by the release's verified support matrix, not by the presence of a binary.

The bundle owns `source/`; do not edit it or place build outputs there.
`MOLT_PROJECT_ROOT` selects your project, while the launcher selects the bundled
`MOLT_SOURCE_ROOT`. Mutable build/cache state lives outside those source inputs.
The bootstrap verifies the wheel and dependency inputs before asking uv to sync.
Release/interpreter-specific environments live under `$MOLT_HOME/environments`;
warm launches reuse them. No activation or `MOLT_VENV` override is needed.
Uninstall a portable installation by removing its bundle, PATH entry, and its
environment under `MOLT_HOME`; uv's download cache is independently managed by uv.

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

Failed checks include a `level` and optional `advice` list in `data.checks`.

## Common failures (doctor)

- **python**: install Python 3.12+ and reopen your terminal.
  - macOS: `brew install python@3.12`
  - Windows: `winget install Python.Python.3.12`
  - Linux: install Python 3.12+ via your package manager
- **uv** (recommended): install uv.
  - macOS: `brew install uv`
  - Windows: `winget install Astral.Uv` or `scoop install uv`
  - Linux: `curl -LsSf https://astral.sh/uv/install.sh | sh`
- **cargo/rustup**: install Rust toolchain and ensure PATH is updated.
  - macOS: `brew install rustup` or use the signed installer from <https://rustup.rs/>.
  - Linux: use the distribution package or the signed installer from <https://rustup.rs/>.
  - Windows: `winget install Rustlang.Rustup`
  - Then: `source $HOME/.cargo/env` (macOS/Linux) or reopen your terminal (Windows)
- **clang**: install a C toolchain.
  - macOS: `xcode-select --install`
  - Linux: `sudo apt-get update && sudo apt-get install -y clang lld`
  - Windows: `winget install LLVM.LLVM` and set `CC=clang`
- **wasm-target** (optional): `rustup target add wasm32-wasip1`
- **uv.lock** / **uv.lock_fresh**: run `uv sync` or `uv lock`
- **molt-runtime**: run `cargo build --release --package molt-runtime`

## Optional environment overrides

- `MOLT_HOME`: override the data/build root (defaults to `~/.molt` unless the bundle is writable)
- `MOLT_PROJECT_ROOT`: overrides project root resolution
