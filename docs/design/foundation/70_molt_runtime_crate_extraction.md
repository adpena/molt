# 70. `molt-runtime` Crate Extraction Contract

`molt-runtime` is the dominant runtime fan-in crate. It should keep wiring,
link-owned ABI exports, and lifecycle ownership, while coherent runtime
subsystems move into lower-layer crates that compile independently.

## Current Contract

- `runtime/crate_graph.toml` is the graph authority. Runtime satellites sit at
  layer 3 and may depend on `molt-runtime-core` / `molt-obj-model`; the layer-4
  `molt-runtime` crate fans them in.
- A split is valid only when it moves one real subsystem authority, deletes the
  old in-crate module tree, wires the feature/dependency owner, and adds the
  crate to the graph ratchet.
- Public widening must be exact. No blanket `pub(crate)` to `pub`; no back-edge
  from a satellite to `molt-runtime`.
- The proof for each split is a focused satellite check plus a real
  `molt-runtime` consumer check. Non-build gates do not prove the compiler can
  still link the runtime.

## Reserved Surfaces

Do not use this document as permission to touch frozen or in-surgery lanes.
`docs/agent/ORCHESTRATION.md` controls live ownership.

- `runtime/molt-cpython-abi/**` and static-extension ABI layout are
  orchestrator/subagent-owned.
- `runtime/molt-runtime/src/cpython_abi_hooks.rs` remains in the runtime fan-in
  until the import/module-state and C-extension lanes explicitly reopen it.
- `runtime/molt-runtime/src/object/**`, the native-call lane, and
  module-state/import surfaces are not extraction targets while the board marks
  them in surgery.

## Landed Satellite Pattern

Existing satellites include text, regex, math, graphlib, difflib, logging,
path, collections, itertools, compression, crypto, net, asyncio, serial, HTTP,
stringprep, XML, ipaddress, zoneinfo, protobuf, and Tk. The canonical pattern is:

1. Create `runtime/molt-runtime-X` with workspace edition/rust-version.
2. Move the subsystem source into that crate; do not leave a duplicate module
   body in `molt-runtime`.
3. Move feature-owned third-party dependencies to the new crate when the
   subsystem owns them.
4. Depend on the satellite from `molt-runtime` and re-export the satellite only
   as the runtime-facing authority.
5. Add the crate to `runtime/Cargo.toml`, root `Cargo.toml` when appropriate,
   and `runtime/crate_graph.toml`.
6. Prove the satellite and the fan-in consumer.

## 2026-07-07 C1-A: VFS Extraction

`runtime/molt-runtime-vfs` owns the mount-oriented virtual filesystem:

- `bundle`, `caps`, `dev`, `file`, `snapshot`, `tmp`, and the VFS root API.
- `vfs_bundle_tar` now belongs to `molt-runtime-vfs`; `molt-runtime` forwards
  the feature instead of owning `tar`.
- `molt-runtime` re-exports the crate as `vfs`, preserving one VFS authority for
  state, IO, runpy, and sys consumers.

The only intentional visibility widening is `load_vfs()`: runtime state still
decides when VFS is lazily loaded, while the VFS crate owns how the state is
constructed.

## Next Decomposition Order

1. Continue legal subsystem extractions that avoid reserved lanes, starting with
   runtime support surfaces that have narrow call sites and no object/module
   back-edge.
2. Extract async runtime only after its task/object/state dependencies are
   mapped and the split can avoid a `molt-runtime` back-edge.
3. When the board opens `object/**`, sever object-to-builtins back-edges and
   extract the object/core authority before carving `builtins`.
4. Split `builtins` by CPython module family only after core/object ownership is
   stable and the feature graph can tree-shake each family.
