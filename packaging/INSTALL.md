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

1. Put the `bin/` directory on your `PATH`.
2. Run `molt doctor` to verify toolchains.
3. Build and run:

```bash
molt build examples/hello.py
~/.molt/bin/hello_molt
```

## Optional environment overrides

- `MOLT_HOME`: override the data/build root (defaults to `~/.molt` unless the bundle is writable)
- `MOLT_VENV`: override the bootstrap venv path
- `MOLT_PROJECT_ROOT`: overrides project root resolution
