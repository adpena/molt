# Toolchains (macOS + Linux)

## Recommended baseline
- CMake + Ninja
- LLVM/Clang (for LLVM backend experiments)
- Rust (for runtime components + WASM + package implementations)
- Python 3.12+ for tooling and tests (Molt targets 3.12+ semantics only; do not support <=3.11).

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
- Install CMake + Ninja: `winget install Kitware.CMake` and `winget install Ninja-build.Ninja`
- Ensure `clang`, `cmake`, and `ninja` are on PATH.

WASM targets:
- `rustup target add wasm32-wasip1 wasm32-unknown-unknown`
- Ensure a WASI sysroot is available for `wasm32-wasip1` builds. Set `WASI_SYSROOT` or
  `WASI_SDK_PATH` if auto-detection is unavailable on your system.

## Platform Pitfalls
- **macOS SDK/versioning**: Xcode CLT must be installed; if linking fails, confirm `xcrun --show-sdk-version` works and set `MACOSX_DEPLOYMENT_TARGET` for cross-linking.
- **macOS arm64 + Python 3.14**: uv-managed 3.14 can hang; install system `python3.14` and use `--no-managed-python` when needed (see `docs/spec/STATUS.md`).
- **Windows toolchain conflicts**: avoid mixing MSVC and clang in the same build; keep one toolchain active.
- **Windows path lengths**: keep repo/build paths short; avoid deeply nested output folders.
- **WASM linker availability**: `wasm-ld` and `wasm-tools` are required for linked builds; use `--require-linked` to fail fast.
