Checkpoint: 2026-01-30T03:54:40Z
Git: 49511fbd80b49acf0c35444f66e847dce7690630 (dirty)

Summary
- Implemented Node/WASI socket host bindings in `run_wasm.js` with worker-backed sockets, io_poller readiness, DNS/service lookup, and structured error handling.
- Added a browser WASM host harness (`wasm/browser_host.html` + `wasm/browser_host.js`) with direct-link/linked loading and capability-gated stubs.
- Added browser DB host adapter for WASM (fetch/JS adapter) with stream headers, Arrow IPC handling, and cancellation polling wired into `loadMoltWasm` imports.
- Refined browser DB host request handling (query vs exec dispatch, pointer validation, adapter payload normalization).
- Updated STATUS/ROADMAP/DB specs to reflect browser DB host support and adjusted wasm DB parity notes.

Files touched (uncommitted)
- CHECKPOINT.md
- docs/AGENT_LOCKS.md
- docs/spec/STATUS.md
- ROADMAP.md
- tools/wasm_link.py
- wasm/browser_host.html
- wasm/browser_host.js
- tests/test_wasm_browser_socket_host.py
- tests/test_wasm_browser_db_host.py
- Large pre-existing dirty tree remains; see `git status -sb` for full list.

Docs/spec updates needed?
- None (STATUS/ROADMAP updated).

Tests
- Not run.

Benchmarks
- Not run.

Profiling
- None.

Known gaps
- Exception hierarchy mapping still uses Exception/BaseException fallback (no full CPython hierarchy).
- `__traceback__` remains tuple-only; full traceback objects pending.
- `str(bytes, encoding, errors)` decoding not implemented (NotImplementedError).
- `print(file=None)` uses host stdout if `sys` is not initialized.
- File I/O gaps: broader codecs + full error handlers (utf-8/ascii/latin-1 only), partial text-mode seek/tell cookies, detach/reconfigure, Windows fileno/isatty parity.
- WASM host hooks for remaining file methods (detach/reconfigure) and parity coverage pending.
- WASM browser host now supports WebSocket-backed stream sockets + DB host adapter, but UDP/listen/server sockets and broader host I/O remain unsupported.
- WASM `str_from_obj` does not call `__str__` for non-primitive objects.
- Backend panic for classes defining `__next__` without `__iter__` (see ROADMAP TODO).
- `sys.argv` decoding still uses lossy UTF-8/UTF-16 until filesystem-encoding + surrogateescape parity lands.
- Pointer registry lock contention optimization still pending (see OPT-0003).

CI
- Last green: https://github.com/adpena/molt/actions/runs/21060145271.
