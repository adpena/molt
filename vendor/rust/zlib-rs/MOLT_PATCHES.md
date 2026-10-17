# Maintained zlib-rs 0.6.8 source

This is altered upstream source, maintained by Molt for cumulative-counter
wrapping. The compression and decompression algorithms are unchanged.

Published package: https://static.crates.io/crates/zlib-rs/zlib-rs-0.6.8.crate

Archive SHA-256: `b268e58e7c693d7c271f93ffc4ba3b380412554231c85bf61ca7af91042a4112`,
verified against both selected workspace locks before path replacement. The
complete published package is retained except its library `Cargo.lock`; the
Molt root and standalone runtime-fuzz locks select dependencies. The upstream
`LICENSE` and other original package files are unchanged except the four source
files identified below and the normalized `Cargo.toml` lint table. Tests use the
upstream crate's own libtest target and dev-dependencies; there is no separate
test harness or vendored lock.

The normalized `Cargo.toml` sets the rustc `unused` group and `clippy::all` to
`allow` at priority -1; `unsafe_op_in_unsafe_fn` stays `deny`. Cargo caps a
registry copy's lints, but this workspace member is linted like Molt source, so
`cargo clippy -D warnings` failed on unmodified upstream code: an unused NEON
import and a redundant `unsafe` on aarch64, and unused C-allocator shims,
same-width casts and a doc-comment blank line on wasm32 (HF-318). Fixing those
lines would widen the patch beyond the counter repair. `Cargo.toml.orig`
remains the unmodified archive source.

The latest published version checked was 0.6.8. Upstream main
`7909c0fc48f5d29f6610770e31d6f0c924f9ec3b` still has the same missing counter
wraps. Its earlier `0063298780f48f30d5470137e5ba4390c29185ee` deflate fix already
wraps windowed input and gives the stable API independent u64 accounting, but
leaves the paths below unchanged. The archive checksum identifies the vendored
release; the main revision identifies the separately inspected upstream state.

- `src/inflate.rs`: checksum completion's private output total, ordinary input
  accounting, and sync recovery's input total wrap at their declared widths.
- `src/deflate.rs` and `src/deflate/algorithm/stored.rs`: pending output and direct
  stored input use the same wrapping rule as existing windowed input and direct
  output. Debug-only cumulative bit counts wrap, including byte alignment; those
  fields feed diagnostics, not capacity or algorithm control.
- `src/stable.rs`: public cumulative u64 totals preserve their wider range on
  32-bit C-counter targets and wrap only at 2^64. They are still computed from
  each operation's bounded pointer progress, independently of the C totals.

This follows zlib's unsigned cumulative accounting, including its low-32-bit
gzip ISIZE field. Buffer capacities, pointer bounds, checksums, lengths, stream
layout, reset/copy ownership, and allocation policy do not change. In particular,
no consumer resets totals to avoid overflow. Runtime cost adds no storage,
allocations, scans, or dispatch; checked builds no longer panic at the declared
counter width. Performance and target qualification are separate from this
source claim.

The `counter_wrap` libtests seed existing counters near their boundaries and
process small literal streams. They check checksum completion in one and two
calls, valid and invalid gzip ISIZE, sync preservation, level-zero direct and
compressed window/pending paths, reset, and the stable API's 32-/64-bit
boundaries. `counter_wrap_debug_bit_counts_and_alignment` additionally requires
`ZLIB_DEBUG`; the checksum test excludes the intentionally checksum-disabled
internal fuzz feature. These are explicit feature coordinates.

Both the root workspace and `runtime/molt-runtime/fuzz` patch crates.io to this
same directory. Existing source-closure and compiler-payload owners include
selected vendored bytes. The provider-selection regression must keep the actual
flate2 and libz-rs-sys dependency closures on this source, rather than merely
building an unused patched workspace member. Retire this patch only when the
selected upstream provider fixes the whole counter family and the narrow
counter controls plus actual compression consumers pass against that source.
