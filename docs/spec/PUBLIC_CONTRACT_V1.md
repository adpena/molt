# Molt v1 Public Stable Contract

Status: executable contract. The two files under `config/` are the authority;
this page explains them and never restates their content.

| Half | File | Checked by |
|---|---|---|
| Declaration (reviewed) | [`config/public_contract_v1.toml`](../../config/public_contract_v1.toml) | `python tools/public_contract_gate.py --check` |
| Surface snapshot (generated) | [`config/public_contract_v1.surface.json`](../../config/public_contract_v1.surface.json) | same gate; refreshed only by `--update` |

## What the contract covers

- **CLI commands and their argument trees.** Every command the `molt`
  entrypoint exposes carries a stability tier in the declaration; the gate
  refuses an untiered command, so no command can ship without a stability
  decision. The snapshot records each command's full argparse surface (flags,
  destinations, arity, choices, scalar defaults, required-ness).
  Scalar defaults here are parser values before configuration resolution. A
  `null` marks an omitted option when the request resolver must distinguish
  omission from an explicitly supplied value; it does not promise a null
  effective compilation setting. The shared build request resolver owns those
  effective settings. For example, omitted `--precompile` and `--wasm-profile`
  resolve to `false` and `auto` without a deployment profile, while an explicit
  `--wasm-profile auto` overrides a deployment profile's `pure` default. The
  snapshot records their omission markers and the owning contract tests verify
  these effective defaults and explicit-option precedence together.
- **Target Python range.** The supported target versions are generated from
  `src/molt/target_python.py` into `molt.release_matrix` and snapshotted.
- **Release targets.** Operating system, architecture, Rust target, and archive
  format per release target, from the same generated matrix. There is no second
  release-target table.
- **Verified-subset matrix identity.** The digest of the exact
  `tools/verified_subset.py matrix`, so a change to the claimed product matrix
  is a visible contract change.
- **Native callable ABI tokens.** The `molt.*_v1` callable contracts from
  `runtime/native_callable_abi.toml`, including the browser embed
  `molt.forward_f32_v1` signature.
- **Public artifact and receipt schemas.** The identifiers a downstream
  consumer may parse: release supply chain, candidate, and manifest documents,
  the release-exit bundle, the Pact acceptance receipt, and the phase-exit,
  legacy-inventory, and public-contract documents themselves.
  The gate reads identifiers from their executable producers and compares them
  with the reviewed declaration. A snapshot update cannot bless a stale or
  undeclared schema; producer, declaration and generated surface must agree.

## Opt-in linker input capture

`molt build --capture-link-inputs` retains development/admission metadata: the
linker's input buffers and a content observation published with the output.
It defaults to off and forces a fresh final link, including a full reproducer
write proportional to the consumed inputs. Ordinary builds do not create this
capture.

Original-emission capture covers native Cranelift/LLVM and WASM producers,
including one-shot compilation, daemon responses and cache generations that
retain their original observation. Native final-link capture requires ELF LLD;
WASM requires `--linked` and WASM LLD. JIT, factgraph, object-only, transpiler and
MLIR capture remain refused. An older bytes-only cache entry has unknown origin
and cannot grant capture. Shared stdlib archives retain the original observations
of their constituents. These source contracts do not widen the release-target
or Python-version matrices; installed cold/warm and platform execution remain
separate qualification obligations.

The release-consumer receipt requires retained compilation evidence, and its
receiver rechecks the raw linker inputs, output publication and original emission
identity. Capture alone does not verify the compiler's loaded process image,
authenticate a kernel receipt, establish performance admission or satisfy H0.
It never sets `compiled_with_verified`. The
[toolchain contract](areas/tooling/0001-toolchains.md#explicit-final-link-input-capture)
owns the evidence joins and remaining admission obligations; recording this
option in the stable build surface does not certify those obligations.

## Tiers

| Tier | Promise |
|---|---|
| `stable` | Surface is kept within a major version. Changing it requires `--update` of the snapshot in the same landing, which is the reviewable act of changing the contract. |
| `preview` | Exposed and snapshotted so drift is visible, but the surface may change between minor releases. |
| `internal` | Repository apparatus (proof queue, build servers, harnesses) reachable through the same entrypoint. No compatibility promise. |

## Versioning and release gating

- Every advertised command and nested action requires end-to-end evidence on
  its applicable OS/architecture/CPython cells, with native/WASM coverage where
  relevant. Parser snapshots, mocks, unit tests and successful `--help` calls
  cannot substitute for executing the real consumer. Success, invalid inputs,
  unsupported gates, failure/exit status, cancellation and authorization paths
  are part of the command contract. Destructive or publishing commands use
  isolated test destinations, never real user installations or public registries.
  Ordinary invocations may inspect readiness but may not implicitly install
  tooling, edit PATH or remove competing installations; setup must explain its
  changes and require explicit authorization. These are release acceptance
  obligations: the current surface gate and installed native/WASM smoke
  consumer do not yet establish this complete command matrix, and smoke success
  is not verified-subset determinism. Use the existing release/phase
  authorities for closure, not a parallel checklist.
- Releases follow semver. `molt --version` and the wheel version are the
  `pyproject.toml` project version resolved through `molt._version`.
- A `v1.x.y` tag is a stable release. Source admission checks the E1-E4 bundle,
  which refuses PASS while the findings ledger at the source commit holds an
  open row; planning names the required `H0` artifact but does not certify
  phase exit.
  The signing job prepares and signs H0, and
  `tools/release/release_authority.py index` requires that authenticated green
  manifest for the tagged commit (see `docs/design/CENTURY_SYSTEMS_PLAN.md` §5
  and `tools/phase_exit_manifest.py`). Pre-1.0 tags carry no stability promise
  and need no H0 phase exit, but still require E1-E4 evidence. Their bundle
  records and reports open findings, which do not hold a pre-1.0 release.
- Deprecation is a time-bounded public transition: the superseded lane is
  registered in `config/legacy_inventory.toml` with its replacement and a
  `removal_release`, and `legacy_count` must be zero before a phase exit is
  green. There is no silent removal and no indefinite compatibility shim.

## Explicit exclusions

The [Python contract](areas/compat/contracts/verified_subset_contract.md)
distinguishes intended semantic scope from verified implementation. Its
[dynamic-semantics policy](areas/compat/contracts/dynamic_execution_policy_contract.md)
restricts unrestricted `exec`/`eval`/`compile`, arbitrary runtime monkeypatching
and reflection that obstruct efficient compilation. Scoped mutation and
introspection retain their admitted Python semantics. Missing implementations,
conformance failures and performance defects remain release obligations; they
do not create exclusions. Compiled binaries never fall back to a host Python
installation.

## Changing the contract

1. Change the code.
2. Run `python tools/public_contract_gate.py --check`; it names the command (and
   its tier) or the surface field that moved.
3. If the change is intended, run `--update` and land the snapshot with the
   code. A `stable` change lands only in a major release line; a `preview`
   change in a minor.
4. The proof plan rule `public-contract` runs the gate and its tests on every
   touched authority file.
