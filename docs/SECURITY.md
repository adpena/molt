# Molt Security & Hardening

Security claims apply to an explicitly verified target, backend, runtime profile,
Python version and host configuration. The
[public stable contract](spec/PUBLIC_CONTRACT_V1.md) and
[release acceptance matrix](../config/release_acceptance_matrix.toml) own the
release obligations. Implemented policy and available audit tools alone do not
establish those obligations.

## Capability and host boundaries

The [capability contract](CAPABILITIES.md) owns permission resolution and native
and WASM host checks. Build and run default to the generated ambientless tier;
explicit grants are resolved through that same policy. Capability checks apply
at the operations that implement them. They do not isolate arbitrary native
code, foreign extensions or host callbacks from the authority of their process.

Filesystem permission tokens authorize access. Path confinement requires the
virtual mount or host adapter described by the capability contract; a path list
reduced to a token does not establish confinement. WASM isolation depends on the
selected engine and admitted host imports. An embedding host is responsible for
the authority and validation of its callbacks.

## Memory, extension and resource boundaries

Rust ownership, collection checks and object-layout invariants are part of the
implementation. Unsafe Rust, FFI, allocator and generated-code paths require
verification at their actual consumers. No blanket memory-safety or audited-unsafe
claim follows from the implementation language.

Molt supplies a maintained runtime C API and a bounded `Python.h` source facade.
The [extension ABI contract](spec/areas/compat/contracts/libmolt_extension_abi_contract.md)
separates source recompilation, stable ABI declarations and CPython binary
compatibility. Header or symbol coverage alone does not prove extension execution,
lifetime safety or package support.

[Resource controls](RESOURCE_CONTROLS.md) describes the configuration and tracker
API, including incomplete allocation coverage and independent thread-local
budgets. Complete memory, time, allocation, recursion and operation-size
enforcement remains V1-19 release work. Configuration is not evidence of an
enforced aggregate budget. A deployment requiring hard limits must establish
those limits through its operating system, engine or host.

Developer and compiler RSS guards supervise development subprocesses. Their
accounting, cleanup and costs belong to that apparatus; they do not establish
resource limits in an emitted user binary. Runtime enforcement must be qualified
on the actual native and WASM execution cells.

## Supply chain and reproducibility

Use the selected lockfiles and
[toolchain custody contract](spec/areas/tooling/0001-toolchains.md) for dependency
and executable admission. Locking dependencies does not establish signer identity
or prevent vulnerabilities in admitted code. Capability-manifest digests and
package checksums establish integrity only within their declared coverage;
authenticity requires the release trust policy and validated signatures.

The [packaging contract](../packaging/PACKAGING.md) owns immutable
candidate assembly and provenance. The release workflow builds independent native
and runtime generations and passes them to candidate assembly. Reproducibility
requires their actual comparison and the complete release gates to pass for the
selected source and toolchain. It is not an unconditional bit-identical-output
promise.

## Verification

Differential tests use CPython as the semantic oracle. They must exercise the
actual compiled consumer and compare observable behavior; they do not by
themselves prove absence of security defects.

`tools/runtime_safety.py` provides sanitizer, Miri and fuzzing commands. The
[sanitizer workflow](../.github/workflows/sanitizers.yml) configures ASan and
Miri lanes; its ASan runs disable leak detection and its TSan lane is unwired.
The [proof plan](agent/PROOF_PLAN.generated.md) selects dependency and audit
obligations. A configured command counts as evidence only after it passes with
its exact source, toolchain, target and inputs. Unsupported or unexecuted cells
remain open in the [release findings](agent/V1_HANDOFF_FINDINGS.md).

## Reporting a vulnerability

Please report vulnerabilities privately to the project owner, **@adpena** on
GitHub, rather than opening a public issue. The project aims to acknowledge a
report within 24 hours and provide a fix within 7 days; these are response goals,
not a guaranteed remediation deadline.
