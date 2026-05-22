# Attribute IC Ref Ownership Plan

## Design
- Treat every `AttrICEntry` slot as owning all heap-backed fields it will release:
  `name_bits`, `result_bits`, and `class_bits`.
- Centralize entry retain/release so cache insertion, replacement, and shutdown
  cannot drift.
- Keep borrowed lookup contracts intact: `class_attr_lookup_raw_mro` remains
  borrowed; the result IC explicitly retains what it stores.

## Files
- `runtime/molt-runtime/src/builtins/attributes.rs`
- `tests/test_native_async_shutdown_refcount.py`

## Tests
- Rust focused test for attribute runtime state ownership and replacement cleanup.
- Native compiled asyncio shutdown regression that fails before the ownership fix.
- Focused benchmark recheck for the stale generated failure list:
  `bench_async_await.py`, `bench_channel_throughput.py`,
  `bench_dict_comprehension.py`, `bench_import_time.py`,
  `bench_parse_msgpack.py`, `bench_procedural_gen.py`, and
  `bench_ptr_registry.py`.

## Exit Criteria
- No refcount underflow after minimal compiled `asyncio.run`.
- The stale generated benchmark failure list becomes green on a focused rerun.
- Focused Rust/Python tests pass under canonical guard roots.
