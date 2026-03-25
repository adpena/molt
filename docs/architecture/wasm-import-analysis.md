# WASM Import Analysis: Pure-Computation Module Optimization

**Date:** 2026-03-06
**Target:** `vertigo/site/world_engine/generator.wasm` (13.1 MB, 90 imports)
**Tool:** `tools/wasm_strip_unused.py`

## Summary

The Molt WASM compiler emits a monolithic binary that imports the full host function surface regardless of what the Python source actually uses. For pure-computation modules (math, json, print — like `generator.py`), **60 of 90 imports (66.7%) are never called** at runtime and are stubbed as no-ops in the browser host (`molt-wasm-host.ts`).

## Import Breakdown by Category

| Category | Count | Status | Notes |
|---|---|---|---|
| **Essential** (args, clock, random, proc_exit, sched_yield) | 8 | KEEP | Required for any WASI module |
| **IO / stdout** (fd_write, fd_read, fd_close, fd_seek, prestat) | 7 | KEEP | Required for `print()` |
| **Indirect call dispatch** (molt_call_indirect0..13) | 14 | KEEP | Required for Python function pointers |
| **Table** (__indirect_function_table) | 1 | KEEP | Required for indirect calls |
| **Filesystem** (path_open, path_rename, fd_readdir, etc.) | 11 | STRIPPABLE | Returns ERRNO_NOSYS/ERRNO_BADF in browser |
| **Process** (spawn, kill, wait, terminate, stdio, poll) | 9 | STRIPPABLE | Returns 0/-1 in browser |
| **Database** (exec, query, poll) | 3 | STRIPPABLE | Returns 0 in browser |
| **WebSocket** (connect, send, recv, close, poll) | 5 | STRIPPABLE | Returns 0 in browser |
| **Socket** (29 functions: bind, listen, accept, send, recv, etc.) | 29 | STRIPPABLE | Returns 0/-1 in browser |
| **Time/timezone** (timezone, local_offset, tzname) | 3 | STRIPPABLE | Returns 0 in browser |
| **TOTAL** | **90** | | **60 strippable** |

## Strippable Imports (Full List)

### Process (9)
- `molt_process_write_host`, `molt_process_close_stdin_host`, `molt_process_terminate_host`
- `molt_getpid_host`, `molt_process_kill_host`, `molt_process_wait_host`
- `molt_process_spawn_host`, `molt_process_stdio_host`, `molt_process_host_poll`

### Database (3)
- `molt_db_exec_host`, `molt_db_query_host`, `molt_db_host_poll`

### WebSocket (5)
- `molt_ws_recv_host`, `molt_ws_send_host`, `molt_ws_close_host`
- `molt_ws_connect_host`, `molt_ws_poll_host`

### Socket (29)
- All 29 `molt_socket_*` functions plus `molt_os_close_host` and `molt_socket_poll_host`

### Time (3)
- `molt_time_timezone_host`, `molt_time_local_offset_host`, `molt_time_tzname_host`

### Filesystem / WASI (11)
- `fd_readdir`, `fd_filestat_get`, `fd_filestat_set_size`
- `path_open`, `path_rename`, `path_readlink`, `path_unlink_file`
- `path_create_directory`, `path_remove_directory`, `path_filestat_get`
- `poll_oneoff`

## Overhead Assessment

### Binary size impact: Minimal (~2.4 KB)
Each import entry in the WASM binary is approximately 40 bytes (module name + function name + type reference). 60 strippable imports account for roughly 2,400 bytes — negligible relative to the 13.1 MB total binary. The binary has no debug/name sections to strip either.

### Browser host impact: Moderate (~120 lines of stub code)
The `molt-wasm-host.ts` file contains ~120 lines of no-op stub definitions for these 60 imports. Removing them would simplify the host significantly.

### Instantiation overhead: Low but non-zero
WebAssembly instantiation must resolve all imports. 60 extra import resolutions add measurable but small overhead to startup. On a cold browser load of a 13 MB module, parsing dominates.

### Real optimization opportunity: Compiler-side dead code elimination
The true win is in the Molt compiler (`molt-backend/wasm.rs`). If the compiler tracked which host functions a Python module actually references and omitted unreachable host call wrappers, wasm-opt/wasm-tools could then tree-shake the associated code sections. This would reduce not just imports but the function bodies that wrap those imports — potentially saving tens of KB of compiled code.

## Usage

```bash
# Analyze imports
uv run tools/wasm_strip_unused.py path/to/module.wasm

# JSON output for CI/tooling
uv run tools/wasm_strip_unused.py path/to/module.wasm --json

# Strip debug sections and produce trimmed copy
uv run tools/wasm_strip_unused.py path/to/module.wasm --strip -o /tmp/trimmed.wasm
```

## Recommendations

1. **Short-term:** Keep the current browser host stubs. The binary overhead is negligible and the stubs ensure any WASM module works regardless of what Python imports it uses.

2. **Medium-term:** Add a `--pure-computation` flag to `tools/wasm_link.py` that emits internal wasm no-op functions for strippable categories, removing the need for the browser host to provide them. This simplifies the host and makes the module more portable.

3. **Long-term:** Implement dead-import elimination in `molt-backend/wasm.rs`. Track which `molt_*_host` functions are actually reachable from the compiled Python code and omit unreachable ones from the import section entirely. Run `wasm-opt` with `--remove-unused-module-elements` as a post-link step.
