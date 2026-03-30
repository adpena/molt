# Per-Function Compilation Caching — Design

## Goal

Replace the monolithic stdlib `.o` cache with per-function object caching.
Each compiled function gets its own cached `.o` file. Builds compile only
functions not already in the cache, then link the needed `.o` files.

## Current Architecture (problems)

```
IR (user + stdlib) → Backend (compile ALL) → output.o → link → binary
```

- Compiles ALL functions every time (unless monolithic stdlib cache matches exactly)
- Monolithic cache breaks when import set changes (different function indices)
- First build: 60-220s depending on import set
- Incremental: 1.6s IF import set is identical to cache, otherwise full rebuild

## Target Architecture

```
IR → Backend:
  for each function F in IR:
    key = hash(F.name + F.ir_content + backend_version)
    if cache/key.o exists:
      skip compilation
    else:
      compile F → cache/key.o
  link: ld -r cache/f1.o cache/f2.o ... → output.o → link → binary
```

- First build: ~60s (compile all, cache each)
- ANY subsequent build: ~1-2s (only new/changed functions compile)
- Import set changes: only NEW functions compile
- No index mismatch: each function is self-contained with relocations

## Implementation

### Phase 1: Per-function `.o` emission (backend change)

The backend currently compiles all functions into one ObjectModule and emits
one `.o`. Change to: compile each function into its OWN ObjectModule, emit
per-function `.o` files, then `ld -r` merge at the end.

Key change in `runtime/molt-backend/src/lib.rs`:
- After TIR optimization, for each function:
  - Compute content hash: `hash(function_name + ops_content)`
  - Check `MOLT_FUNCTION_CACHE_DIR/hash.o`
  - If hit: skip compilation, add path to link list
  - If miss: compile to temp ObjectModule, finalize, write `.o`, add to link list
- Final: `ld -r` all per-function `.o` files → `output.o`

### Phase 2: Cache directory management (CLI change)

- `MOLT_FUNCTION_CACHE_DIR` env var → `~/.molt/cache/functions/`
- CLI creates the directory, passes it to the backend
- LRU eviction: when cache exceeds `MOLT_FUNCTION_CACHE_MAX_MB` (default 500MB),
  evict oldest-accessed `.o` files

### Phase 3: Content-based invalidation

- Each cached `.o` is keyed by: `sha256(function_name + ir_ops_json + backend_binary_hash)`
- Backend binary hash ensures recompilation when the compiler changes
- IR ops hash ensures recompilation when the function's code changes

## Non-Goals

- Cross-machine cache sharing (would need deterministic compilation)
- Remote cache (out of scope for now)
- Parallel compilation of individual functions (already batched)
