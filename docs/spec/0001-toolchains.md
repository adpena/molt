# Toolchains (macOS + Linux)

## Recommended baseline
- CMake + Ninja
- LLVM/Clang (for LLVM backend experiments)
- Rust (for runtime components + WASM + package implementations)

## macOS
- Install Xcode CLT: `xcode-select --install`
- Homebrew recommended: `brew install llvm cmake ninja pkg-config`

## Linux (Ubuntu/Debian)
- `sudo apt-get install -y cmake ninja-build pkg-config llvm clang lld`

Rust via rustup:
- `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

WASM targets:
- `rustup target add wasm32-wasip1 wasm32-unknown-unknown`
