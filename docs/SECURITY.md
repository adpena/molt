# Molt Security & Hardening

Molt is built on the principle of **Secure by Default**. We treat Python as a high-level specification that is lowered into a hardened, capability-gated native runtime.

## 1. Core Security Pillars

### A. Memory Safety (Rust)
The Molt runtime and compiler are written in Rust with **minimal, audited `unsafe` blocks** (e.g., arena allocation and NaN-boxing internals), verified via tooling.
- **No CPython C-extension ABI (yet)**: Molt does not ship a `Python.h`-compatible ABI today; the primary path is a `libmolt` C-API subset with recompiled extensions, and any CPython bridge is planned to be capability-gated and opt-in (TODO(c-api, owner:runtime, milestone:SL3, priority:P2, status:missing): define `libmolt` C-extension ABI surface + bridge policy).
- **NaN-Boxing Invariants**: Our 64-bit object model uses strict NaN-boxing. We maintain pointer invariants (48-bit addresses) and sign-extension checks to prevent pointer manipulation attacks.
- **Runtime Guards**: Molt inserts bounds checks on dynamic collection accesses; specialized paths use guards and fall back to runtime checks when safety cannot be proven.

### B. Capability Gating (No Ambient Authority)
Molt employs a **Capability-based Security** model. A Molt binary has zero authority to interact with the OS unless explicitly granted.
- **Explicit Manifests**: Capabilities like `net`, `fs.read`, `env.read`, and `time` must be declared in build flags or capability profiles.
- **Granular Access**: Filesystem access is path-restricted.
- **WASM Isolation**: When targeting WASM, the sandbox enforces these boundaries via the host interface.

### C. Supply Chain & Provenance
- **Lockfile Enforcement**: `molt build` strictly enforces `uv.lock`. If dependencies change without a lockfile update, the build fails.
- **Verified Packages**: Molt Packages (`.moltpkg`) use checksum verification to ensure that the code you run is the code you built.
- **Deterministic Binaries**: Molt guarantees bit-identical output for the same source and toolchain. This allows for "Reproducible Builds," where third parties can verify that a binary matches its public source code.

## 2. Threat Model

### What Molt Protects Against:
1.  **Arbitrary Code Execution (ACE)**: Via memory safety and WASM isolation.
2.  **Data Exfiltration**: Via strict network/filesystem capability gating.
3.  **Dependency Confusion/Substitution**: Via lockfile and checksum enforcement.

### What Molt Does NOT (Currently) Protect Against:
1.  **Logic Errors in Python Source**: If your Python code has a vulnerability (e.g., SQL injection), Molt will faithfully compile that vulnerability into native code.
2.  **Resource Exhaustion (DoS)**: While we have recursion limits, we do not yet have strict memory/CPU quotas for native binaries (though WASM targets can be restricted by the host) (TODO(security, owner:runtime, milestone:RT2, priority:P1, status:missing): memory/CPU quota enforcement for native binaries).

## 3. Verification & Auditing

Molt uses **Differential Testing** as a security tool. By running test cases against both CPython and Molt, we ensure that our "Performance Optimizations" do not introduce "Semantic Divergence" (which is often where security bugs hide).

### Standardized Security Checks:
We use `tools/runtime_safety.py` to run:
- **Sanitizers (ASan/TSan/UBSan)**: For memory, threading, and undefined behavior detection.
- **Miri**: To verify the soundness of our (minimal) Rust `unsafe` blocks.
- **Fuzzing**: Targeted `cargo fuzz` runs for high-risk components (string parsers, codec decoders).

### Supply-chain audits (recommended before release)
- **Rust**: `cargo audit`, `cargo deny check`
- **Python**: `uv run pip-audit`

## 4. Reporting a Vulnerability

If you find a security issue in Molt, please do not open a public issue.
Contact the project owner (**@adpena** on GitHub) privately.
We aim to acknowledge all reports within 24 hours and provide a fix within 7 days.
