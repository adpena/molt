# Stdlib Object Partition Design

## Goal

Implement option `3/C`: a multi-artifact stdlib partition pipeline for native
builds, with explicit partition metadata, daemon/default-path support, and
deterministic cache/link invalidation.

## Current Grounded Status

The codebase now carries the minimum parity contract for the current single
stdlib sidecar model:

- native subprocess backend compiles receive both `MOLT_ENTRY_MODULE` and
  `MOLT_STDLIB_OBJ`;
- backend daemon requests carry the same values through per-request env
  metadata;
- cache mode/versioning and the user-owned symbol boundary are already in place.

The remaining gap is the actual multi-artifact end state: versioned partition
roots/manifests, explicit artifact-list linking, and a real `emit=obj`
contract.

## Why The Single-Object Split Is Not Enough

The existing backend already has a dormant split path, but the single
`MOLT_STDLIB_OBJ` model is not the right end state.

The current codebase shows four hard constraints:

- native builds default to the backend daemon, so backend-subprocess-only env
  tweaks are insufficient;
- the final native link must receive partition artifacts explicitly rather than
  reading ambient process env;
- `emit=obj` still needs a coherent artifact contract;
- cache keys must encode partition mode so old monolithic objects cannot be
  reused under the split pipeline.

## Design

### 1. Partition Artifact Contract

Native non-wasm builds get a versioned stdlib partition root derived from the
backend cache identity. The root contains:

- a manifest describing partition mode/version and the stdlib artifact list;
- one or more stdlib object files, typically batched;
- no user object payload.

`output.o` remains the user object. The stdlib partition root becomes the
canonical sidecar artifact that the daemon, subprocess backend, and native link
all agree on.

### 2. Backend Behavior

The Rust backend keeps only true user/runtime ABI roots in the user object:

- `molt_main`
- entry trampoline/init roots
- `molt_isolate_import`
- `molt_isolate_bootstrap`

Stdlib `molt_init_*` bodies live in the stdlib partition artifacts instead of
being blanket-kept in `output.o`.

On first compile for a given partition root, the backend emits multiple stdlib
batch objects and records them in the manifest. On subsequent compiles for the
same partition root, it compiles user code only.

### 3. Daemon / Subprocess Parity

The daemon request protocol must carry the partition root and the effective
entry-module identity explicitly. The one-shot subprocess path must receive the
same values through its env.

This is a hard requirement: the default native path must use the same partition
contract as the fallback path.

### 4. Native Linking

The native linker receives the explicit stdlib partition artifact list from the
manifest/root, not from ambient env. Link fingerprints hash all linked stdlib
partition artifacts.

For `emit=obj`, the contract cannot silently drop the stdlib sidecar. Either:

- disable partition mode for `emit=obj`, or
- perform a partial relink so the requested object remains complete.

Option `3/C` prefers the second path, but that is only acceptable if the
partial-link behavior is deterministic and tested.

### 5. Cache Versioning

Backend cache keys and daemon cache keys must encode partition mode/version.
Changing from monolithic to partitioned output must force a cache miss even if
the IR is otherwise unchanged.

## Verification

Verification should prove:

- backend ownership excludes non-entry stdlib init roots;
- daemon request payload includes partition-root metadata;
- subprocess fallback receives the same partition metadata;
- native link fingerprints change when any linked stdlib partition artifact
  changes;
- `emit=obj` behavior is explicitly tested for the chosen contract.

## Non-Goals

- wasm partitioning;
- dynamic runtime discovery of partition artifacts;
- undocumented fallback to monolithic output when partition metadata is missing.
