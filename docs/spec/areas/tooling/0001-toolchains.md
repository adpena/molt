# Toolchains (macOS + Linux)

## Recommended baseline
- CMake + Ninja
- LLVM/Clang (for LLVM backend experiments)
- A complete LLVM distribution with `llvm-config` matching the Rust
  `inkwell` feature pinned in `runtime/molt-backend/Cargo.toml`.
- Rust (for runtime components + WASM + package implementations)
- Python 3.12+ for tooling and tests (Molt targets 3.12+ semantics only; do not support <=3.11).
- Cargo-hosted DX helpers: `wasm-tools`, `wasm-pack`, and `cargo-edit`
  (`cargo-upgrade`) for dependency sweeps.

## macOS
- Install Xcode CLT: `xcode-select --install`
- Homebrew recommended: `brew install llvm cmake ninja pkg-config`
- WASM sysroot (for `wasm32-wasip1` builds): `brew install wasi-libc`

## Linux (Ubuntu/Debian)
- `sudo apt-get install -y cmake ninja-build pkg-config llvm clang lld`

Rust via rustup:
- `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

## Windows
- Install Visual Studio Build Tools (MSVC) or full Visual Studio.
- Install LLVM/Clang: `winget install LLVM.LLVM`
- The LLVM backend specifically needs `llvm-config.exe`; some Windows LLVM
  installers include `clang`/`wasm-ld` but omit `llvm-config`. Those installs
  are useful for native/WASM linking but are not a complete Rust LLVM backend
  toolchain. Build a matching MSVC LLVM developer prefix with:
  `python tools/bootstrap_llvm.py --version 22.1.8 --prefix target\toolchains\llvm-22.1.8`.
  Set the matching `LLVM_SYS_<ver>_PREFIX` (for example
  `LLVM_SYS_221_PREFIX`) to the LLVM prefix containing `bin\llvm-config.exe`.
- Install CMake + Ninja: `winget install Kitware.CMake` and `winget install Ninja-build.Ninja`
- Ensure `clang`, `llvm-config`, `cmake`, and `ninja` are on PATH.
- Run source LLVM builds from an x64 Visual Studio developer shell, or let
  `tools/bootstrap_llvm.py` activate `VsDevCmd.bat` from an installed Build
  Tools instance.

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
