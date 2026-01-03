# Addendum: Integrating `uv` and Rust tooling in Molt

This addendum can be appended to the Gemini prompt and/or Molt’s spec. It clarifies how we want to leverage **uv** for Python project
management and **Rust** (Cargo + rustup) for the runtime, WASM interop, and “Molt Packages” implementations—while keeping the project
**free of CPython C-extensions**.

---

## 1) `uv` integration (Python tooling, lockfiles, reproducibility)

### Goals
- **Fast, deterministic** dependency resolution and installs for dev + CI.
- A **single source of truth** lockfile for Python-side tooling and any Python-hosted components (compiler front-end utilities, test harness, etc.).
- A developer workflow that is **one-command**: create env, install, run lint/tests.

### Proposed approach
- Support both pip/venv and uv, but make **uv the recommended path** when available.
- Keep dependencies in `pyproject.toml` (PEP 621). Use uv to generate/maintain a lockfile.
- Use uv for:
  - creating/managing venv
  - installing dev dependencies quickly
  - executing tools (`ruff`, `mypy`, `pytest`) reproducibly

### File conventions
- `pyproject.toml` remains authoritative for Python dependencies.
- Add a lockfile:
  - `uv.lock` (preferred by uv)
- Add a small “tool runner” shim:
  - `tools/dev.py` or `tools/dev.sh` that uses uv when present, else falls back to venv.

### CLI workflow (examples)
- Local dev:
  - `uv venv` (or `uv sync`)
  - `uv run ruff check .`
  - `uv run mypy src`
  - `uv run pytest`
- CI:
  - Cache uv downloads
  - `uv sync --frozen` (or equivalent) to guarantee exact lock usage

### Spec requirement
Molt should define **reproducible dev and CI environments** with a “frozen” mode:
- If lockfile exists: installs must be exact (`--frozen` behavior)
- If lockfile missing: the build should fail in CI (policy decision), or generate lock as a separate PR step

### Bootstrap script requirement
Enhance `bootstrap_molt.sh` to:
- detect `uv`
- if present, prefer:
  - `uv venv`
  - `uv pip install -r ...` or `uv sync`
- else fallback to `python -m venv` + pip

---

## 2) Rust toolchain integration (runtime, backends, packages, WASM)

### Goals
- Use Rust for:
  1) **Molt micro-runtime** (strings/containers/exceptions/memory model)
  2) selected compiler components where appropriate (e.g., IR passes, codegen)
  3) “Molt Packages” implemented in Rust for performance + portability
- Promote a **WASM interop story** that is stable and portable across macOS/Linux.

### Rust installation & targets
- Use `rustup` as the canonical installation mechanism.
- Minimum targets:
  - native host target(s)
  - `wasm32-wasip1` and/or `wasm32-unknown-unknown` (depending on chosen ABI)
- Document and enforce an MSRV (minimum supported Rust version) aligned with stable Rust.

### Workspace layout
Adopt a Cargo workspace at repo root:
- `runtime/` as a Rust crate (or crates):
  - `molt-runtime` (core runtime)
  - optionally `molt-wasm-abi` (shared ABI types)
- `compiler/` may be:
  - Python front-end now, Rust later; or hybrid from day 1
- `packages/`:
  - Rust crates producing either:
    - WASM modules (preferred for portability/sandboxing)
    - or static libs linked into the binary (for max perf)
- `tools/`:
  - Rust-based helper CLIs if useful (benchmark harness, packager)

### Build integration options
Pick one primary build orchestration strategy for MVP and document it:

**Option A (recommended for MVP): Python orchestrator + Cargo + LLVM/Cranelift**
- Molt CLI (Python) orchestrates:
  - Rust runtime build via `cargo build`
  - codegen (LLVM/Cranelift) compilation
  - final link step
- Pros: fast to iterate, easy integration with Python-side analysis tooling.
- Cons: split toolchain.

**Option B: Rust-first CLI**
- `molt` CLI in Rust, embedding Python parsing as a library or using a Python parser port.
- Pros: cohesive binaries.
- Cons: more up-front work.

In both options, define:
- artifact directories
- reproducible flags
- caching strategy
- deterministic builds policy

### Rust code quality gates
- `cargo fmt`, `cargo clippy -D warnings`, `cargo test`
- Add a CI workflow job for Rust (separate from Python job).
- Use `cargo deny` or `cargo audit` as a supply-chain gate (recommended).

---

## 3) WASM interoperability: Rust + WASI as the “package ABI”

### Why WASM for packages
- Portable across macOS/Linux
- Sandboxed execution
- Stable ABI surface vs native linking across distros

### Target design
- “Molt Packages” can compile to WASM and be embedded into the final binary.
- Define a narrow ABI:
  - memory allocation strategy
  - strings/bytes passing
  - error/exception mapping
  - deterministic mode (no wall-clock, no random) for pure compute modules

### Tooling
- Recommended:
  - `wasmtime` for host runtime in dev/test
  - `wasm-tools` for validation/optimization
  - `wit-bindgen` (or WIT component model) if you choose that direction

### Policy
- No arbitrary system access by default for WASM modules.
- Explicit capabilities (FS/network) must be opted into.

---

## 4) Practical “ask” to Gemini Pro (copy/paste)

Add the following explicit instructions to the prompt:

1. **uv**:
   - Specify a uv-first dev workflow with lockfile and CI `--frozen` mode.
   - Add uv caching in GitHub Actions.
   - Provide fallback to venv+pip for environments without uv.

2. **Rust**:
   - Add a Cargo workspace and Rust CI job (fmt/clippy/test).
   - Define MSRV, and add supply chain scanning (`cargo audit`/`deny`).
   - Decide MVP build orchestration (Option A or B) and justify.

3. **WASM**:
   - Use Rust→WASM for “Molt Packages” portability.
   - Define ABI (strings/bytes/errors/capabilities) and testing harness.

---

## 5) Concrete acceptance criteria
- `uv sync --frozen` succeeds on a clean machine and reproduces identical environments in CI.
- `cargo test` + `cargo clippy -D warnings` pass in CI.
- A sample “Molt Package” implemented in Rust compiles to WASM and is callable from a Molt-built binary with deterministic output.
