# WASM Import Stripping for Pure-Computation Modules

> **UPDATE 2026-03-20:** Import stripping for freestanding deployment is now implemented via `tools/wasm_stub_wasi.py` (post-link WASI import replacement with unreachable stubs). See `--target wasm-freestanding`.

**Date:** 2026-03-07
**Context:** Molt WASM codegen (`molt-backend/src/wasm.rs`) emits a monolithic import surface. This document describes what is emitted, what is unnecessary for pure-computation modules, and how to strip it.

## 1. Current Import Surface

The compiled `generator.wasm` (13.1 MB) declares **90 imports** across three namespaces:

### `molt_runtime` (internal runtime — ~624 `add_import` calls in wasm.rs)
All runtime host functions are unconditionally registered in `wasm.rs` lines 1006-1800+. These cover:
- **Core:** `runtime_init`, `runtime_shutdown`, `alloc`, `print_obj`, `print_newline`
- **Arithmetic/comparison:** `add`, `sub`, `mul`, `div`, `lt`, `gt`, `eq`, etc.
- **Collections:** `list_*`, `dict_*`, `set_*`, `tuple_*`, `string_*`
- **Object model:** `get_attr_*`, `set_attr_*`, `alloc_class_*`, `closure_*`
- **Async/concurrency:** `async_sleep`, `future_*`, `promise_*`, `thread_*`, `lock_*`, `chan_*`
- **IO/OS:** `process_*`, `socket_*`, `os_*`, `db_*`, `ws_*`

These are imported from the `"molt_runtime"` module namespace. In the final linked binary, they appear as `env.molt_*` host functions.

### `wasi_snapshot_preview1` (WASI syscalls — linked from runtime)
22 WASI functions appear in the final binary, added during linking (not directly in `wasm.rs`):
- **Essential:** `args_sizes_get`, `args_get`, `environ_sizes_get`, `environ_get`, `random_get`, `clock_time_get`, `proc_exit`, `sched_yield`
- **IO/stdout:** `fd_read`, `fd_write`, `fd_seek`, `fd_tell`, `fd_close`, `fd_prestat_get`, `fd_prestat_dir_name`
- **Filesystem:** `path_open`, `path_rename`, `path_readlink`, `path_unlink_file`, `path_create_directory`, `path_remove_directory`, `path_filestat_get`, `fd_filestat_get`, `fd_filestat_set_size`, `fd_readdir`
- **Scheduling:** `poll_oneoff`

### `env` (indirect call trampolines)
14 `molt_call_indirect{N}` trampolines for Python function pointer dispatch, plus the `__indirect_function_table` table import.

## 2. Unnecessary Imports for Pure-Computation Modules

For a module like `generator.py` that only does math, list/dict construction, and `json.dumps` + `print`, **60 of 90 imports (67%) are never called at runtime**:

| Category | Count | Verdict |
|---|---|---|
| Process (spawn, kill, wait, stdio, poll) | 9 | STRIP |
| Database (exec, query, poll) | 3 | STRIP |
| WebSocket (connect, send, recv, close, poll) | 5 | STRIP |
| Socket (29 functions) | 29 | STRIP |
| Time/timezone (3 functions) | 3 | STRIP |
| Filesystem WASI (11 functions) | 11 | STRIP |
| **Total strippable** | **60** | |

The remaining 30 imports (core runtime, arithmetic, IO/stdout, indirect calls, essential WASI) are required.

## 3. Recommended Approach

### Option A: Build flag in `wasm.rs` (compiler-side, recommended)

> **UPDATE 2026-03-20:** This option is now implemented (ddc8ea4c). `--wasm-profile pure` performs compile-time stripping of IO/ASYNC/TIME category imports, emitting `unreachable` for stripped call sites. Combined with `wasm-opt --remove-unused-module-elements` post-link, this achieves 30-50% size reduction for pure-compute modules.

Add a `--wasm-profile` flag with values like `full` (default) and `pure`:
- In `pure` mode, skip the `add_import` calls for process/db/ws/socket/time categories.
- Guard the corresponding `emit_call` sites to emit `unreachable` instead of `call $import_idx` for omitted imports.
- Run `wasm-opt --remove-unused-module-elements` as a post-emit step to DCE any code that transitively referenced stripped imports.

**Pros:** Smallest possible binary; no post-processing dependency. The compiler already knows which categories to emit.
**Cons:** Requires plumbing a new flag through the CLI and codegen context.

### Option B: Post-link tool (`wasm-tools` / `wasm-opt`)

Use existing tooling after compilation:
```bash
# Replace strippable imports with internal no-op stubs, then DCE
wasm-tools strip --delete-name-section module.wasm -o stripped.wasm
wasm-opt stripped.wasm --remove-unused-module-elements -o final.wasm
```
Or use the existing `tools/wasm_strip_unused.py` (already in the repo) which can analyze and strip debug sections.

**Pros:** No compiler changes needed; works on any existing `.wasm`.
**Cons:** Cannot fully remove imports unless replaced with internal stubs first (WASM validation requires all imports to be satisfied). Requires a stub-injection pass.

### Option C: Hybrid (recommended for production)

1. **Short-term:** Continue using browser-side no-op stubs in `molt-wasm-host.ts` (current approach, works today).
2. **Medium-term:** Add `--wasm-profile pure` to the compiler (Option A). This is the cleanest solution since the compiler already has full knowledge of which host functions each Python module references.
3. **Post-link:** Always run `wasm-opt --remove-unused-module-elements` regardless of profile, to catch any additional dead code from the runtime link step.

## References

- Existing analysis: `docs/wasm-import-analysis.md` (2026-03-06)
- Existing strip tool: `tools/wasm_strip_unused.py`
- WASM codegen imports: `runtime/molt-backend/src/wasm.rs` lines 1006-1800+
- Browser host stubs: `strata/` or site `molt-wasm-host.ts`
