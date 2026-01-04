# Molt Security Model

## 1. Threat Model
Molt is designed for:
- **Untrusted Input**: Fast processing of network data with memory safety (Rust runtime).
- **Untrusted Code (WASM)**: Running third-party packages in a sandbox.
- **Supply Chain**: Protecting the build pipeline from malicious dependencies.

## 2. Sandboxing
- **WASM Isolation**: Packages compiled to WASM cannot access memory outside their linear memory space.
- **Capability-based Security**: Every capability (File, Network, Time) must be explicitly granted in the application manifest.
- **Runtime Guards**: Even in native code, Molt inserts bounds checks for all collection accesses (unless proven safe by the compiler).

## 3. Supply Chain Security
- **Lockfile Enforcement**: `molt build` requires a frozen `uv.lock`.
- **Package Verification**: All Molt Packages are verified against a checksum.
- **SBOM Generation**: Mandatory for all production builds.

## 4. Memory Safety
- **Rust Spine**: The runtime is written in safe Rust. `unsafe` is used sparingly and only for performance-critical object manipulation, subject to strict audit.
- **No C-Extensions**: Eliminating the largest source of memory unsafety in the Python ecosystem.

## 5. Operational security checks (implementation)
- Enforce lockfiles (`uv sync --frozen`) in build pipelines.
- Verify package checksums for all Molt Packages.
- Require explicit capability manifests (`molt.toml` or `pyproject.toml`) for I/O, time, and randomness.
- Deny ambient FS/network access by default for WASM modules and FFI boundaries.
- Run security scans (e.g., `cargo audit`) for release builds where available.
