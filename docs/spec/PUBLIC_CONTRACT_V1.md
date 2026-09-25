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
- **Target Python range.** The supported target versions are generated from
  `config/release_targets.toml` into `molt.release_matrix` and snapshotted.
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
- A `v1.x.y` tag is a stable release. Source admission checks the E1-E4 bundle;
  planning names the required `H0` artifact but does not certify phase exit.
  The signing job prepares and signs H0, and
  `tools/release/release_authority.py index` requires that authenticated green
  manifest for the tagged commit (see `docs/design/CENTURY_SYSTEMS_PLAN.md` §5
  and `tools/phase_exit_manifest.py`). Pre-1.0 tags carry no stability promise
  and need no H0 phase exit, but still require E1-E4 evidence.
- Deprecation is a time-bounded public transition: the superseded lane is
  registered in `config/legacy_inventory.toml` with its replacement and a
  `removal_release`, and `legacy_count` must be zero before a phase exit is
  green. There is no silent removal and no indefinite compatibility shim.

## Explicit exclusions

Compiled binaries do not support unrestricted `exec`/`eval`/`compile`, runtime
monkeypatching, or unrestricted reflection, and never fall back to a host
Python installation. These are design boundaries of the product, not gaps.

## Changing the contract

1. Change the code.
2. Run `python tools/public_contract_gate.py --check`; it names the command (and
   its tier) or the surface field that moved.
3. If the change is intended, run `--update` and land the snapshot with the
   code. A `stable` change lands only in a major release line; a `preview`
   change in a minor.
4. The proof plan rule `public-contract` runs the gate and its tests on every
   touched authority file.
