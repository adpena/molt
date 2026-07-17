# Toolchains (macOS, Linux, and Windows)

## Recommended baseline
- CMake + Ninja
- LLVM/Clang/MLIR (for LLVM and MLIR backend development)
- A complete LLVM distribution with `llvm-config` matching the Rust
  `inkwell` feature pinned through `molt.llvm_toolchain` from
  `runtime/molt-backend-native/Cargo.toml`.
- One prefix owns LLVM, MLIR, and TableGen. Molt projects it to every binding
  environment and rejects split prefixes or a mismatched major/minor before a
  build starts.
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

Rust via rustup:
- `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

## Windows
- Install Visual Studio Build Tools (MSVC) or full Visual Studio.
- Install LLVM/Clang: `winget install LLVM.LLVM`
- The LLVM backend specifically needs `llvm-config.exe`; some Windows LLVM
  installers include `clang`/`wasm-ld` but omit `llvm-config`. Those installs
  are useful for native/WASM linking but are not a complete Rust LLVM backend
  toolchain. Build a matching MSVC LLVM/Clang/MLIR developer prefix with:
  `python tools/bootstrap_llvm.py --version 22.1.8 --prefix target\toolchains\llvm-22.1.8`.
  The bootstrap command prints `MOLT_LLVM_PREFIX`, `LLVM_SYS_<ver>_PREFIX`,
  `MLIR_SYS_<ver>_PREFIX`, `TABLEGEN_<ver>_PREFIX`, and `LLVM_CONFIG_PATH`; all
  name the same verified prefix.
- Install CMake + Ninja: `winget install Kitware.CMake` and `winget install Ninja-build.Ninja`
- Ensure `clang`, `llvm-config`, `cmake`, and `ninja` are on PATH.
- Run source LLVM builds from an x64 Visual Studio developer shell, or let
  `tools/bootstrap_llvm.py` activate `VsDevCmd.bat` from an installed Build
  Tools instance.

## Distribution boundary

LLVM/MLIR is a developer and source-build dependency. Shipped Molt binaries
must package the optional MLIR backend executable and the redistributable
runtime libraries it needs, with platform/architecture gating at package build
time. Binary-only end users must not need Cargo, CMake, Ninja, TableGen, or an
LLVM SDK. A source checkout builds the standalone backend once on first MLIR
use through the same manifest-pinned toolchain authority.

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

## Platform Pitfalls
- **macOS SDK/versioning**: Xcode CLT must be installed; if linking fails, confirm `xcrun --show-sdk-version` works and set `MACOSX_DEPLOYMENT_TARGET` for cross-linking.
- **macOS arm64 + Python 3.14**: uv-managed 3.14 can hang; install system `python3.14` and use `--no-managed-python` when needed (see `docs/spec/STATUS.md`).
- **Windows toolchain conflicts**: avoid mixing MSVC and clang in the same build; keep one toolchain active.
- **Windows LLVM backend**: official, winget, and Chocolatey LLVM binaries may
  omit `llvm-config`; do not treat them as satisfying `llvm-sys` until
  `llvm-config --version` reports the required major/minor.
- **Windows path lengths**: keep repo/build paths short; avoid deeply nested output folders.
- **WASM linker availability**: `wasm-ld` and `wasm-tools` are required for linked builds; use `--require-linked` to fail fast.
