# Molt install (binary release)

This bundle includes the Molt CLI, a production-optimized compiler, and matching
compiler/runtime sources. The optional `molt-worker` helper is a separate package
with its own command and data paths; installing both does not duplicate ownership.
Molt requires the local toolchains listed below; it does not install them on
your behalf. Private CLI dependencies require the explicit setup command below.
Binaries produced by `molt build` are expected to run on target machines without
any host Python installation or hidden CPython fallback.

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
- **WASM (optional)**: the [WASM toolchain](../docs/spec/areas/tooling/0001-toolchains.md)
  supplies target-specific C tools and the WASI sysroot. Public
  `molt run --target wasm` also requires Node.js. Keep WASI SDK compilers
  separate from the native C/C++ toolchain; they are not native replacements.

Set `PYTHON` to a CPython executable path to select an interpreter explicitly;
the native launcher on every platform honors the same override.
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

After installation, run `molt setup --install-cli-dependencies` to explicitly
authorize the private CLI dependency environment. Ordinary invocations only check
its readiness: they never install, remove or repair dependencies. The command
reports the exact source, interpreter, destination and locked dependency inputs.
It changes neither PATH nor another installation and installs no toolchains.

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

The scripts install the bundle under `~/.local/share/molt` (respecting
`XDG_DATA_HOME`) or `%LOCALAPPDATA%\Programs\Molt`. `--prefix` / `-Prefix` selects
another location. This is independent of `MOLT_HOME`, the mutable data root.
Shell-profile or user-PATH edits require `--add-path` / `-AddPath`; without that
option use the printed absolute executable path or adjust PATH yourself.

1. Review and run `molt setup --install-cli-dependencies` to authorize CLI dependencies.
2. Run `molt doctor` to inspect toolchains and installation ambiguity.
   Builds and readiness checks never add Rust targets automatically. If a target
   is missing, review and run the setup command in the diagnostic, then retry.
3. Build and run with an explicit output:

```bash
molt build examples/hello.py --output hello_molt
./hello_molt
```

## Verification checklist

`molt build app.py --profile dev` and `--profile release` use the same production
compiler. `--diagnostics` distinguishes its profile and content identity from
the program/runtime profiles. Native/WASM feature availability remains governed
by the release's verified support matrix, not by the presence of a binary.

The bundle owns `source/`; do not edit it or place build outputs there.
Project discovery starts from the entry file; set `MOLT_PROJECT_ROOT` only to
override it explicitly. The launcher selects the bundled `MOLT_SOURCE_ROOT`.
Mutable build/cache state lives outside those source inputs.
The native launcher embeds the bootstrap and executes only `source/src`; there
is no second installed Molt wheel or copied executable bootstrap in the bundle.
The separately published wheel remains a distinct installation option, not an
additional source of CLI code inside a binary installation.
The bootstrap verifies dependency and home-policy inputs before asking uv to
check readiness. Only explicit setup may synchronize the private environment.
Release/interpreter-specific environments live under `$MOLT_HOME/environments`;
warm launches reuse them. No activation or `MOLT_VENV` override is needed.
`MOLT_HOME` defaults to the CLI cache root's `home` directory: typically
`~/.cache/molt/home` (respecting `XDG_CACHE_HOME`) or `%LOCALAPPDATA%\Molt\home`.
It must be outside the immutable installation prefix.
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

Expected: JSON output, exit code 0 when required tools are available. Prefer
`--output` when selecting an executable destination; `$MOLT_BIN` otherwise
defaults to `$MOLT_HOME/bin`.

`doctor` reports the active source and Python executable, ordered PATH candidates,
and competing installations without running the alternatives or changing them.
Symlinks and hardlinks to the same executable are not separate installations.
Warnings are diagnostic, not permission to uninstall. Choose an explicit path or
adjust PATH, and use the original package manager only after confirming which
installation is unwanted. Molt does not guess package ownership from directory names.

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
- **uv** (required): install uv.
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
- **CLI dependencies**: review and run `molt setup --install-cli-dependencies`.
  Do not rewrite a packaged compiler's sealed lockfiles.
- **molt-runtime**: run `cargo build --release --package molt-runtime`

## Optional environment overrides

- `MOLT_HOME`: override the mutable data/build root, not the installation prefix
- `MOLT_PROJECT_ROOT`: overrides project root resolution
