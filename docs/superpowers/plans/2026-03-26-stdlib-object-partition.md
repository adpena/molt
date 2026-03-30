# Stdlib Object Partition Residual Completion Plan

> Audited on 2026-03-30. The original implementation plan is stale: partition-mode cache identity, daemon/subprocess partition metadata plumbing, and focused verification already landed. This document now tracks only the remaining closure work.

## Audit outcome

- Already landed:
  - partition metadata propagation through daemon and subprocess compile paths;
  - partition-aware cache variant support in `src/molt/cli.py`;
  - focused CLI verification for the partition plumbing.
- Still incomplete:
  - a hard proof that backend symbol ownership excludes non-entry stdlib `molt_init_*` symbols;
  - explicit native link preparation and link fingerprint coverage for partition artifacts;
  - the final `emit=obj` contract under partition mode.

## Parallel tracks

### Track P1 - Backend ownership boundary (independent)

- Add or tighten the Rust test for `user_owned_symbol_whitelist_keeps_only_entry_roots` in `runtime/molt-backend/src/main.rs`.
- Prove that true entry/runtime ABI roots remain user-owned while non-entry stdlib roots stay outside the user-owned symbol set.
- Validation:
  - `cargo test -p molt-backend --features native-backend user_owned_symbol_whitelist_keeps_only_entry_roots -- --nocapture`

### Track P2 - Explicit native link inputs and fingerprinting (independent)

- Add or refresh CLI tests proving native link preparation includes explicit stdlib partition artifacts.
- Keep `_link_fingerprint()` driven by explicit artifact content and artifact membership, not ambient environment state.
- Validation:
  - `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k 'stdlib_link_fingerprint or stdlib_partition_mode_changes_cache_identity'`

### Track P3 - `emit=obj` contract (depends on P2)

- Choose one canonical behavior for partition mode under `emit=obj`:
  - partial-link the sidecar stdlib artifacts into the emitted object, or
  - raise an explicit unsupported error.
- Add a focused test that locks the chosen contract.
- Update `docs/OPERATIONS.md` if the user-facing behavior changes.
- Validation:
  - `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k stdlib_partition_emit_obj`

## Exit gate

- `cargo test -p molt-backend --features native-backend user_owned_symbol_whitelist_keeps_only_entry_roots -- --nocapture`
- `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py`
- Any contract change is reflected in `docs/OPERATIONS.md` in the same change.
