# Molt Security Model

## Contract and verification scope

[`docs/SECURITY.md`](../../../SECURITY.md) describes the current security
boundaries. Security acceptance applies to the declared target, backend, profile,
Python version and host configuration in the
[release acceptance matrix](../../../../config/release_acceptance_matrix.toml).
Implemented checks, headers and configured audit commands do not establish a
passing release cell.

## Required authorities

- [Capability policy](../../../CAPABILITIES.md) owns grants, denial, effects and
  native/WASM host boundaries. Filesystem confinement belongs to the selected
  mount or host adapter. Foreign code and callbacks retain their process or host
  authority.
- [Resource controls](../../../RESOURCE_CONTROLS.md) owns configuration and
  tracker limitations. Complete aggregate runtime enforcement remains V1-19
  release work; developer RSS guards do not enforce an emitted guest budget.
- [The extension ABI contract](../compat/contracts/libmolt_extension_abi_contract.md)
  owns stable C API declarations and the bounded CPython source facade.
  Extension admission requires actual compilation, link, execution and lifecycle
  evidence for its claimed coordinate.
- [Packaging](../../../../packaging/PACKAGING.md) and
  [toolchain custody](../tooling/0001-toolchains.md) own immutable inputs,
  provenance, reproducibility and artifact trust. Checksums provide integrity
  within their declared coverage; release authentication requires the trust
  policy and validated signatures.

## Verification obligations

The [sanitizer workflow](../../../../.github/workflows/sanitizers.yml) owns the
configured ASan and Miri lanes, including their explicit limitations. The
[proof plan](../../../agent/PROOF_PLAN.generated.md) selects dependency and audit
checks. Unsafe Rust, FFI, generated code, allocation rollback and capability
boundaries require tests at their real consumers. CPython differential tests
prove the exercised semantic behavior; sanitizers, Miri, fuzzing and dependency
audits provide their own scoped evidence. Unsupported, failed and unexecuted
cells remain explicit in the [release findings](../../../agent/V1_HANDOFF_FINDINGS.md).
