# Melior 0.27.0 provenance

- Published source: crates.io `melior` 0.27.0
- Upstream: `https://github.com/mlir-rs/melior`
- Portability patch source: local commit `88f3aa10` based on upstream
  `b74f3a3f`
- License: Apache-2.0 (`LICENSE` in this directory)

This exact crate source is the MLIR Rust API dependency used by Molt's excluded
standalone `runtime/molt-backend-mlir` workspace. The four-file portability
patch removes host ABI assumptions that treated bindgen C-enum aliases as
always unsigned. LLVM's C API permits the generated integer alias to differ
across toolchains; conversions now preserve the raw alias type and validate
values without changing Melior's public semantic enums.

Keeping the source beside the standalone workspace makes builds reproducible
on Windows, Linux, and macOS without an absolute checkout path or mutation of
Cargo's registry cache. `runtime/molt-backend-mlir/Cargo.toml` remains the sole
dependency-selection authority for this excluded workspace.
