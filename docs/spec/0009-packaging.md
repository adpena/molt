# Molt Packaging & Distribution

## 1. Goal: Single-File Executables
Molt aims to produce a single, statically-linked executable for the target platform.
- **Static Linking**: The `molt-runtime` and all "Native" Molt Packages are linked into the final binary.
- **WASM Embedding**: WASM-based packages are embedded as bytes in the data segment and instantiated at runtime.

## 2. Cross-Compilation
Molt leverages Rust's excellent cross-compilation support.
- **Toolchain**: `molt build --target x86_64-unknown-linux-musl`.
- **Zig as Linker**: Molt uses `zig cc` as a cross-platform linker to avoid glibc versioning hell on Linux.

## 3. Reproducibility
- **Deterministic Builds**: Given the same `uv.lock` and source, Molt produces bit-identical binaries.
- **Build ID**: Every binary contains a unique hash of its source closure and compiler version.

## 4. Signing & SBOM
- **Signing**: Support for `codesign` (macOS) and `cosign` (Linux) builtin to the CLI.
- **SBOM**: Molt generates a Software Bill of Materials (CycloneDX or SPDX) including:
    - The compiler version.
    - All Python dependencies (from `uv.lock`).
    - All Molt Packages (WASM or Native).
    - The Rust toolchain version.

## 5. Molt Registry
A centralized (but cacheable) registry for verified Molt Packages.
- **Trust**: Only packages signed by the Molt team or trusted vendors are allowed by default in Tier 0.
