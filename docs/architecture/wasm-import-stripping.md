# WASM Import Stripping for Pure-Computation Modules

> **UPDATE 2026-03-20:** Import stripping for freestanding deployment is now implemented via `tools/wasm_stub_wasi.py` (post-link WASI import replacement with unreachable stubs). See `--target wasm-freestanding`.
> **UPDATE 2026-06-29:** Runtime import discovery is no longer a monolithic
> `wasm.rs` scan or a pre-emission dependency table. The generated ABI manifest
> owns import names/types, loop runtime calls, host exports, fallback exports,
> and split/link output export policy consumed by `tools/wasm_link_format.py`.
> `wasm/module_abi/runtime_surface.rs` remains the single IR-scanning planner
> for module ABI facts. Auto and Pure register the profile-allowed import
> surface, code emission records actual import accesses in `TrackedImportIds`,
> and finalization strips unobserved imports for relocatable and non-relocatable
> output before relocation/linking sections are emitted.

**Date:** 2026-03-07
**Context:** Molt WASM codegen (`runtime/molt-backend-wasm/src/wasm.rs`) now routes import
surface decisions through generated ABI data and the module-level runtime surface
planner. This document records the import-stripping contract and the legacy
motivation for pure-computation modules.

## 1. Current Import Surface

The compiled `generator.wasm` (13.1 MB) declares **90 imports** across three namespaces:

### `molt_runtime` (internal runtime)
Runtime host functions are declared from the generated ABI registry
(`runtime/molt-backend-wasm/src/wasm_abi_generated/`). Full profile registers
the whole registry for process-host compatibility. Auto and Pure profile
register the profile-allowed registry, then `TrackedImportIds` records the
imports that emitted code actually addresses. Finalization remaps function
indices, preserves padded LEB operand width for relocatable calls, strips
unobserved imports, and only then writes relocation/linking sections. The
generated registry covers:
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

### Option A: Compiler-side profile/import planning (recommended)

> **UPDATE 2026-06-28:** Pure profile now uses the same runtime-surface planner
> as Auto instead of registering a broad "core" registry. It emits only observed
> runtime imports, then applies the generated process/IO/time skip prefixes
> fail-closed. Split-runtime browser kernels therefore ask the shared runtime to
> export the actual app import surface, not a duplicate broad pure lane.

Use the `--wasm-profile` flag with values `auto` (default), `full`, and `pure`:
- In `auto` mode, plan imports from observed IR and strip unused host imports before
  the app/runtime export check.
- In `pure` mode, plan imports from observed IR and skip process/db/ws/socket/time
  categories in the module ABI import planner.
- In `full` mode, preserve the whole generated host-import registry for hosts that
  intentionally provide every import.
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

- Existing analysis: `docs/architecture/wasm-import-analysis.md` (2026-03-06)
- Existing strip tool: `tools/wasm_strip_unused.py`
- WASM import registry and host export/fallback data:
  `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml`
- WASM split/link export keep policy:
  `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml`
- WASM runtime surface planner: `runtime/molt-backend-wasm/src/wasm/module_abi/runtime_surface.rs`
- WASM emitted-use import ledger: `runtime/molt-backend-wasm/src/wasm_import_tracking.rs`
- WASM function-index remapping: `runtime/molt-backend-wasm/src/wasm_binary/code_remap.rs`
- Browser host stubs: `strata/` or site `molt-wasm-host.ts`
