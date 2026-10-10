# Molt install (binary release)

This bundle includes the Molt CLI, a production-optimized compiler, prebuilt
native and WASM runtime artifacts for every supported profile, stdlib tier and
extension/freestanding variant, and the matching compiler/runtime sources. The optional `molt-worker` helper is a separate package
with its own command and data paths; installing both does not duplicate ownership.
Molt requires the local toolchains listed below; it does not install them on
your behalf. Private CLI dependencies require the explicit setup command below.
Binaries produced by `molt build` are expected to run on target machines
without any host Python installation or hidden CPython fallback.

## Requirements

- **CPython 3.12+** available as `python3` (or `python` on Windows);
  Python 3.14 hosts require **3.14.1+**. Version 3.14.0 emits compiler warnings
  during AST-only parsing and cannot preserve Molt target-version diagnostics.
- **uv** manages a binary bundle's private CLI dependencies directly from the
  bundled `uv.lock`, including artifact hashes and Python/platform markers.
  Platform wheels use the pip environment and do not require uv for normal
  compilation.
- **No Rust toolchain.** The compiler and runtime ship prebuilt; `molt build`,
  `molt run`, `molt doctor` and `molt setup` neither use nor install Rust. A
  request outside the shipped runtime cells (for example a GPU feature flag or
  a cross-compilation target) fails and lists the shipped cells. Building other
  runtime configurations is Molt development in a source checkout
  (`MOLT_SOURCE_ROOT`). Upgrade an installed Molt with the installer, package
  manager or pip that provided it; `molt update --no-locks` only provisions
  its pinned wasm-tools. Rust toolchain refresh (`molt update`) is a
  source-checkout development workflow.
- **C/C++ toolchain** (links programs against the shipped runtime):
  - macOS: Xcode Command Line Tools (`xcode-select --install`)
  - Linux: clang/llvm + build essentials
  - Windows: LLVM clang or set `CC` to a compatible compiler, plus the MSVC
    runtime libraries and Windows SDK (Visual Studio Build Tools C++ workload)
- **WASM (optional)**: the [WASM toolchain](../docs/spec/areas/tooling/0001-toolchains.md)
  supplies target-specific C tools and the WASI sysroot. Public
  `molt run --target wasm` also requires Node.js. Keep WASI SDK compilers
  separate from the native C/C++ toolchain; they are not native replacements.

Set `PYTHON` to a CPython executable path to select an interpreter explicitly;
the native launcher on every platform honors the same override.
The bootstrap checks the CPython 3.12+ minimum; project package admission and
the source frontend also exclude CPython 3.14.0. Verified versions and target
support remain those listed in the release matrix. Source parsing uses this
interpreter: to target Python 3.N, the CLI must run on CPython 3.N or newer.
Homebrew binds its frontend to Python 3.14 for the current 3.12-3.14 policies.
After changing `PYTHON` for a binary bundle, authorize the corresponding private
environment with `molt setup --install-cli-dependencies`, then retry the build.
This frontend requirement does not create a Python dependency in compiled guests.

## Install

### pip

Install a platform wheel downloaded from a release with
`pip install <wheel-path>`. The workflow produces wheel assets; it currently
has no PyPI publication step for the planned `pip install molt` registry route.
A platform wheel installs this same distribution (compiler, runtime cells and signed source
manifest) into the environment's `share/molt/distribution`; the `molt`
command uses it directly without uv or Rust. On other platforms pip selects
the pure-Python wheel, whose CLI requires `MOLT_SOURCE_ROOT` to name a Molt
source checkout; it never adopts a nearby checkout implicitly.

Installing Molt does not register pytest plugins or take over other projects'
test runs. Molt's repository loads its development test guard explicitly through
its own pytest configuration.

Project dependency commands use the nearest user project from the current
directory, or the explicit `MOLT_PROJECT_ROOT`. They reject sealed compiler
inputs as a project. `molt install` manages that project's `.molt-venv` using
uv and the host platform's environment layout. Without package arguments, or
with `--sync`, it exports the uv project lock (including source mappings)
and resolves additional requirements before removing packages outside the
combined transitive closure. Requirements files retain their includes, constraints, hashes and relative-path rules.

Dependency resolution currently uses the CLI interpreter and host platform,
not the selected guest version or WASM platform. Build discovery also admits a
project `.venv/lib/python*/site-packages` before the managed `.molt-venv`; this
POSIX-only path can shadow managed packages. A target-bound dependency
environment and consistent module-root admission remain release blockers.

`molt install add <package>` persists the dependency and synchronizes the same
environment through one uv project operation; a persistence or installation
failure returns an error. Explicit package and requirements paths are relative
to the invoking directory. `molt deps` reads the project's dependency metadata;
`molt vendor` resolves its lock and writes default or relative output paths under
the project root. Python-only projects do not require a compiler Cargo lock.
Installing Molt itself through pip, a package manager, or a release bundle is a
separate operation. These dependency-command checks do not replace the
[installed release acceptance](PACKAGING.md).

The separate `test`, `bench`, `clean`, `lint`, and `profile` implementations
still select the compiler-input tree as their execution or cleanup root. Their
installed behavior is an open product boundary; they do not provide a verified
user-project workflow yet.

### Package managers

Use the commands below when the corresponding release is available in the
package repository. A generated manifest or template in this source tree does
not establish that a package has been published.

Homebrew (macOS/Linux):

```bash
brew tap adpena/molt
brew install molt
```

The formula projects the complete bundle directories from the distribution
authority, retaining `runtime/` and hidden files under `source/`. Its package
test compiles a release-profile argument-binding guest and compares the
standalone executable with CPython. Template and filesystem checks do not
establish a successful Homebrew installation; POSIX package acceptance remains
required.

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
compiler. `--diagnostics` reports the selected backend, target, guest profile,
runtime Cargo profile and compiler Cargo profile. The required combinations come
from [`release_acceptance_matrix.toml`](../config/release_acceptance_matrix.toml).
Installed selection requires the exact shipped runtime cell and compiler
capabilities; declaring a required lane does not make an unavailable backend
ready. Native and LLVM use the same native runtime bytes for an identical
runtime profile, while their compiled programs remain distinct products.
The production compiler includes LLVM through the source-pinned static SDK build
path. Installed builds select those admitted compiler bytes and do not discover,
provision or execute `llvm-config`; the ordinary native C/link tool requirements
still apply. The LLVM notice is retained under `share/molt/LLVM-LICENSE.TXT` and
in the platform wheel. Actual platform/backend qualification remains governed
by the release's verified support matrix.

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
Use the installer or package manager that provided Molt to uninstall it. For a
portable installation, remove its bundle and any PATH entry you added. Private
environments under `MOLT_HOME/environments`, retained runtime generations under
`MOLT_HOME/installed-runtime`, and other build/cache outputs remain separately
owned mutable data. Remove them only when no retained installation or build uses
them; do not remove unrelated outputs. uv manages its download cache separately.

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
      {"name": "python", "ok": true, "detail": "3.12.x at <python> (requires CPython 3.12+ (Python 3.14 requires 3.14.1+))"},
      {"name": "uv", "ok": true, "detail": "<path-to-uv>"},
      {"name": "molt-runtime", "ok": true, "detail": "<count> shipped runtime cells verified under <runtime-root>"}
    ]
  },
  "warnings": [],
  "errors": []
}
```

Failed checks include a `level` and optional `advice` list in `data.checks`.

## Common failures (doctor)

- **python**: install a [supported CPython host](#requirements) and reopen your terminal.
  - macOS: `brew install python@3.12`
  - Windows: `winget install Python.Python.3.12`
  - Linux: install a supported CPython version via your package manager
- **uv** (binary bundles): install uv for their private CLI environment.
  - macOS: `brew install uv`
  - Windows: `winget install Astral.Uv` or `scoop install uv`
  - Linux: `curl -LsSf https://astral.sh/uv/install.sh | sh`
- **clang**: install a C toolchain.
  - macOS: `xcode-select --install`
  - Linux: `sudo apt-get update && sudo apt-get install -y clang lld`
  - Windows: `winget install LLVM.LLVM` and set `CC=clang`
- **CLI dependencies**: review and run `molt setup --install-cli-dependencies`.
  Do not rewrite a packaged compiler's sealed lockfiles.
- **molt-runtime**: `molt doctor` verifies the shipped runtime cells. Reinstall
  Molt if one is damaged; installed Molt never rebuilds them.

## Optional environment overrides

- `MOLT_HOME`: override the mutable data/build root, not the installation prefix
- `MOLT_PROJECT_ROOT`: overrides project root resolution


Release verification keeps frontend/build CPython separate from emitted guest
execution. The release consumer gate retains native and WASM post-uninstall
replay in a sealed Linux root with exact pinned runtime inputs and no host Python
or package installation. Source implementation alone is not qualification:
Linux engine/ptrace replay and the other platform filesystem adapters remain
release acceptance work. See [the packaging contract](PACKAGING.md) for the
current boundary, input provisioning and retained evidence.
