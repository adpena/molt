# Toolchains (macOS, Linux, and Windows)

## Recommended baseline
- CMake + Ninja
- LLVM/Clang/LLD/MLIR/Polly (for LLVM and MLIR backend development)
- A complete LLVM distribution with `llvm-config` matching the Rust
  `inkwell` feature pinned through `molt.llvm_toolchain` from
  `runtime/molt-backend-native/Cargo.toml`.
- One SDK prefix owns LLVM, Clang, LLD, MLIR, Polly, and TableGen. Molt projects
  that SDK identity into each binding's required path shape and rejects split
  identities or a mismatched major/minor before a build starts. `llvm-config`
  may live outside that prefix
  only when its own `--prefix` result proves the same SDK identity, as in
  Debian/Ubuntu's versioned `/usr/bin/llvm-config-<major>` layout. In that
  layout `LLVM_SYS_<ver>_PREFIX=/usr` is the llvm-sys executable-search root,
  while `MOLT_LLVM_PREFIX` and the MLIR/TableGen prefixes remain
  `/usr/lib/llvm-<major>`.
- Every companion executable must report the exact patch release owned by
  `config/llvm_toolchain_releases.toml`; matching only the major/minor is not
  sufficient for an accepted SDK.
- Rust (for runtime components + WASM + package implementations)
- Python 3.12+ for tooling and tests (Molt targets 3.12+ semantics only; do not support <=3.11).
- Cargo-hosted DX helpers: `wasm-tools`, `wasm-pack`, and `cargo-edit`
  (`cargo-upgrade`) for dependency sweeps.

## macOS
- Install Xcode CLT: `xcode-select --install`
- Homebrew recommended: `brew install llvm mlir cmake ninja pkg-config`
- WASM sysroot (for `wasm32-wasip1` builds): `brew install wasi-libc`

## Linux (Ubuntu/Debian)
- `sudo apt-get install -y cmake ninja-build pkg-config llvm clang lld mlir`

Hosted CI does not maintain a parallel package script. The local
`.github/actions/setup-llvm` action has two projections of the same
`molt.llvm_toolchain` authority: `profile=full` verifies the complete
LLVM/MLIR/LLD/Polly SDK, while `profile=wasm,wasi=true` installs only the
manifest release's WebAssembly linker and the pinned WASI sysroot needed by
Rust workspace truth. `config/llvm_toolchain_releases.toml` owns the wasi-sdk
release, LLVM compatibility line, URL, byte size, SHA-256, provenance URL, and
archive root. The action checks the archive size and digest before extraction,
then verifies headers, libc, VERSION, and the exact `wasm-ld` and `llvm-nm`
identities before projecting `MOLT_WASM_LD`, `MOLT_LLVM_NM`,
`MOLT_WASI_SYSROOT`, and `WASI_SYSROOT` to every consumer in the job. WASM
archive inspection consumes only that verified `llvm-nm`; its exact
version/content receipt is part of persistent symbol-cache identity, with no
Rust-toolchain or ambient native `nm` fallback.

`MOLT_LLVM_NM` selects one executable, not a shell command. A selected path or
PATH-resolved name must pass lexical and resolved-content custody before probing;
the captured executable generation is checked again at execution and cache reuse.
Quoted paths preserve spaces and native separators without admitting arguments.

Rust via rustup:
- `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

## Windows
- Install Visual Studio Build Tools (MSVC) or full Visual Studio.
- Install LLVM/Clang: `winget install LLVM.LLVM`
- The LLVM backend specifically needs `llvm-config.exe`; some Windows LLVM
  installers include `clang`/`wasm-ld` but omit `llvm-config`. Those installs
  are useful for native/WASM linking but are not a complete Rust LLVM backend
  toolchain. Build a matching MSVC LLVM/Clang/MLIR developer prefix with:
  `python -m tools.bootstrap_llvm` (or the equivalent direct-script entry
  `python tools/bootstrap_llvm.py`; the exact patch release, URL, size, checksum,
  and provenance come from `config/llvm_toolchain_releases.toml`).
  The bootstrap command prints `MOLT_LLVM_PREFIX`, `LLVM_SYS_<ver>_PREFIX`,
  `MLIR_SYS_<ver>_PREFIX`, `TABLEGEN_<ver>_PREFIX`, and `LLVM_CONFIG_PATH`; all
  name the same verified prefix.
  Release source archives are accepted only when their SHA-256 matches
  `config/llvm_toolchain_releases.toml`; that manifest also owns the exact
  canonical build type. Extraction is archive-bound and rehashes the extracted
  tree before reuse. Extracted sources and installed prefixes share one
  transactional publisher: exclusive process lock, unique staging prefix,
  durable phase journal, and deterministic rollback or completion on startup.
  Canonical path identity is never deletion authority: every nonempty source or
  build tree must carry a valid self-consistent tool-owned marker, otherwise the
  bootstrap fails closed without modifying it. There is no legacy reset lane.
  The CMake cache is keyed by the release record, extracted-source digest, and
  a digest of the architecture/target/project/build configuration. Canonical
  project, target, and build-type sets are exact (no extras) and are bound into
  the published attestation with the release-manifest digest. Publication
  occurs only after the host linker,
  LLVM-C, Clang, LLD, Clang resource headers, LLVM/MLIR/LLD/Polly libraries, and a real
  C++ compile-link probe all pass. Cached validation projects that attested
  proof and forces full hashing whenever NTFS ChangeTime is unavailable.
  `D:\` is retired and rejected for every source, build, download, prefix, and
  environment authority. Unlisted development releases require an explicit
  noncanonical prefix, source URL, and SHA-256; their source/download/build
  custody is derived beside that prefix and cannot overlap canonical managed
  custody.
- Install CMake + Ninja: `winget install Kitware.CMake` and `winget install Ninja-build.Ninja`
- Ensure `clang`, `llvm-config`, `cmake`, and `ninja` are on PATH.
- Run source LLVM builds from an x64 Visual Studio developer shell, or let
  `tools/bootstrap_llvm.py` activate `VsDevCmd.bat` from an installed Build
  Tools instance.

## Python runtime identity

Python environment identity captures one immutable loader snapshot in
`molt.python_native_locations`: the OS-loaded executable, loaded paths, loader
aliases, non-file loader contracts, and observed Mach-O CPU identities. PSAPI
identifies the Windows executable; dyld image zero identifies the macOS main
image. Linux requires the first loader image to agree with `AT_PHDR` and the
kernel mapping's device/inode identity; ambiguous explicit-interpreter launches
fail closed. Configured CPython base/venv launchers remain content-bound inputs,
not invented loaded importers or providers. The receipt designates an observed
executable component and binds that designation into its closure digest.
Dependency capture consumes and
rechecks that same snapshot. On macOS, universal images are read through the
exact slice already selected by dyld, never the first or generic matching slice.
Missing files require an explicit shared-cache contract; census or slice changes
invalidate the capture. This does not relax exact-target binary admission or
claim support for another architecture. Synthetic fixtures project the selected
executable from their realized environment, not the test runner's executable.

## Distribution boundary

LLVM/MLIR is a developer and source-build dependency. Shipped Molt binaries
must package the optional MLIR backend executable and the redistributable
runtime libraries it needs, with platform/architecture gating at package build
time. Binary-only end users must not need Cargo, CMake, Ninja, TableGen, or an
LLVM SDK. A source checkout builds the standalone backend once on first MLIR
use through the same manifest-pinned toolchain authority.
The WASI sysroot and `wasm-ld` follow the same boundary: developers and source
builders need them when producing WASM artifacts; end users running shipped
native or WASM binaries do not.

## MLIR diagnostics

The standalone backend exposes bounded, opt-in developer telemetry without
changing release artifacts or the binary-user dependency boundary:

- `MOLT_MLIR_TRACE_FUNCTIONS=1` reports each function as lowering begins.
- `MOLT_MLIR_ONLY_FUNCTION=<name>` isolates one function from a captured
  SimpleIR module for diagnosis.
- `MOLT_MLIR_OPT_LEVEL=O0|O1|O2|O3` selects the progressive-lowering pipeline
  level for a diagnostic run.
- `MOLT_MLIR_DUMP_DIR=<path>` writes each function's MLIR before verification,
  so a verifier failure retains the exact input that produced it.

These controls are diagnostic projections of the same backend and pass
pipeline. They do not authorize a second compiler path, partial artifact
publication, or a relaxed verifier.

WASM targets:
- `rustup target add wasm32-wasip1 wasm32-unknown-unknown`
- `cargo install wasm-tools --locked`
- `cargo install wasm-pack --locked`
- Ensure a WASI sysroot is available for `wasm32-wasip1` builds. Set `WASI_SYSROOT` or
  `WASI_SDK_PATH` if auto-detection is unavailable on your system.

## Cargo workspace truth custody

The canonical Rust truth runner separates network custody from execution. Root
`Cargo.toml` and `Cargo.lock` own the ordinary compiler/runtime workspace,
including the dependency resolution used by trybuild's offline child. The runner
performs one locked fetch, one locked package-metadata query, and one locked
`--workspace --tests --no-fail-fast` traversal, all with the root manifest.
Intentionally isolated MLIR, fuzz, bootstrap, and probe workspaces are not added
to this traversal. A host target-runner hook gives each
`resource_enforcement` test a fresh process so process-global address-space
limits cannot poison sibling tests or convert an exact failure into an
unattributed SIGABRT. The outer Rust proof receipt uploads the runner's nested
receipt containing root-scoped phase return codes, exact observed red identities,
and the suite-honesty verdict. Binary receipts remain under `binaries/root`, bound
to the run, source snapshot, and exact executable bytes; Cargo metadata and
compiler artifacts own package/target identity and complete binary coverage.
Prefetch or metadata failure publishes a terminal receipt without starting tests.

`molt.cargo_workspace` projects declared members for the harness and structural
checks. Lock validation additionally follows local dependency manifests (including
excluded helpers, target/dev/build dependencies, and workspace inheritance), so a
cached lock check cannot overlook a changed input. Cargo remains the dependency
resolver. Add ordinary crates explicitly to the root members list; independent
workspace exclusions are not a second ordinary runtime workspace.

Root profiles are the only profile authority: compiler `release` retains unwind
support; shipping native runtimes use `release-output`/`release-size`, and WASM
uses `wasm-release`. Select profiles explicitly rather than changing policy by
launching Cargo from a different directory.

## Platform Pitfalls
- **macOS SDK/versioning**: Xcode CLT must be installed; if linking fails, confirm `xcrun --show-sdk-version` works and set `MACOSX_DEPLOYMENT_TARGET` for cross-linking.
- **macOS arm64 + Python 3.14**: uv-managed 3.14 can hang; install system `python3.14` and use `--no-managed-python` when needed (see `docs/spec/STATUS.md`).
- **Windows toolchain conflicts**: avoid mixing MSVC and clang in the same build; keep one toolchain active.
- **Windows LLVM backend**: official, winget, and Chocolatey LLVM binaries may
  omit `llvm-config`; do not treat them as satisfying `llvm-sys` until
  `llvm-config --version` reports the required major/minor.
- **Windows path lengths**: keep repo/build paths short; avoid deeply nested output folders.
- **WASM linker availability**: `wasm-ld` and `wasm-tools` are required for linked builds; use `--require-linked` to fail fast.
