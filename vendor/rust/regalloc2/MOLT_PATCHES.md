# Maintained regalloc2 0.15.2 source

Published package: https://static.crates.io/crates/regalloc2/regalloc2-0.15.2.crate

Archive SHA-256: `757712e8e61590d6d4f5d563483755538b5aa13467837a3b41cd9832509a7f85`.
The archive was checked against this pinned checksum before extraction. The
complete published package is retained except its library `Cargo.lock`; Molt's
workspace lock owns dependency selection. Published manifests, serializer,
statistics and license files are unchanged. Two deprecated primitive-module
imports are removed so the pinned compiler resolves the identical associated
constants without dependency warnings. Upstream revision:
`2fe490bc9dda433f70c54f90f7633ed929f693d9`.

## Scope and invariants

Two changes in Ion's move insertion preserve allocation and move selection:

- The incoming block-parameter cursor advances across blocks within one live
  range. Incoming records are sorted by `(to_vreg, to_block, from_block)`, and
  visited blocks increase within a range. Consumed records cannot match a later
  block. Every range still initializes its cursor from `PrevBuffer`, preserving
  replay for overlapping ranges, duplicate destinations and same-start ranges.
- `InterBlockSources` replaces only the per-vreg inter-block source hash map
  with a dense vector indexed by `Block`. Each occupied slot carries a typed
  `VRegIndex` generation. The owning loop visits each vreg exactly once, so no
  numeric generation counter or wraparound policy is needed. Insertion retains
  the first register allocation, or the latest stack allocation until a
  register appears. Lookup refuses missing sources and stale generations in
  release builds as well as debug builds. The separate block-parameter source
  map is unchanged.

The canonical upstream authorities are
[move insertion](https://github.com/bytecodealliance/regalloc2/blob/2fe490bc9dda433f70c54f90f7633ed929f693d9/src/ion/moves.rs),
[sorted incoming-record production](https://github.com/bytecodealliance/regalloc2/blob/2fe490bc9dda433f70c54f90f7633ed929f693d9/src/ion/liveranges.rs),
and [PR 122's overlapping-range contract](https://github.com/bytecodealliance/regalloc2/pull/122).
The buffer remains intact; this patch does not advance its committed cursor at
each block. [PR 261](https://github.com/bytecodealliance/regalloc2/pull/261) is a
distinct, unmerged generational-vector experiment for a bundle-allocation
HashSet. Its mixed wider benchmark results do not establish a benefit here.

## Cost and proof boundary

For `B` blocks, the source cache lazily allocates and initializes `B` optional
`(VRegIndex, Allocation)` slots at the first source insertion. Functions with no sources allocate no
cache storage. Storage is
`B * size_of::<Option<(VRegIndex, Allocation)>>()`; this can cost more than a
sparse hash map for functions with few populated blocks. Each source insert or
lookup uses one indexed slot and a generation comparison. It removes hashing,
map growth, and repeated full-capacity hash-table clears. There are no extra
syscalls, locks, serialization passes, environment reads or per-vreg allocations.

For a live range visiting `K` blocks and consuming `P` incoming records, cursor
work is `O(K + P)`, including terminating probes, instead of repeatedly scanning
consumed prefixes. Across-range replay remains necessary and is unchanged.
Neither change adds vectorization or parallelism; both remove repeated serial
work before considering either mechanism.

Focused tests in `src/ion/moves_tests.rs` exercise broad-to-sparse generation
transitions, stale and absent source refusal, and explicit allocation preferences
for Int, Float and Vector registers. Those tests do not qualify whole-compiler
behavior or establish a speedup. Integration qualification additionally owns the
exact captured-input ordered-output comparison, focused regression oracles,
consumer builds and whole-compiler measurements. The full symbolic checker on
the large captured case exceeded its diagnostic budget; its completion is not
claimed. Diagnostic capture/serializer changes
and instrumentation are not part of this maintained crate.

Retire this patch when a pinned upstream release implements both contracts and
passes the owning regression, allocator replay and shared compiler-consumer
proofs with acceptable performance and memory on the claimed target matrix.
Remove the local source selection and vendor source together after those proofs;
do not keep parallel allocator implementations.
