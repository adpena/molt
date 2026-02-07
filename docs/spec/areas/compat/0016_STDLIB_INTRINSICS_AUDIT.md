# Stdlib Intrinsics Audit
**Spec ID:** 0016
**Status:** Draft (enforcement + audit)
**Owner:** stdlib + runtime

## Policy
- Compiled binaries must not execute Python stdlib implementations.
- Every stdlib module must be backed by Rust intrinsics (Python files are allowed only as thin, intrinsic-forwarding wrappers).
- Modules without intrinsic usage are forbidden in compiled builds and must raise immediately until fully lowered.

## Progress Summary (Generated)
- Total audited modules: `110`
- `intrinsic-backed`: `40`
- `intrinsic-partial`: `19`
- `probe-only`: `16`
- `python-only`: `35`

## Priority Lowering Queue (Generated)
### P0 queue (Phase 2: concurrency substrate)
- `socket`: `intrinsic-partial`
- `select`: `intrinsic-backed`
- `selectors`: `intrinsic-backed`
- `threading`: `intrinsic-backed`
- `asyncio`: `intrinsic-partial`

### P1 queue (Phase 3: core-adjacent stdlib)
- `builtins`: `intrinsic-backed`
- `types`: `intrinsic-backed`
- `weakref`: `intrinsic-backed`
- `math`: `intrinsic-partial`
- `re`: `intrinsic-partial`
- `struct`: `intrinsic-backed`
- `time`: `intrinsic-backed`
- `inspect`: `intrinsic-partial`
- `functools`: `python-only`
- `itertools`: `intrinsic-backed`
- `operator`: `intrinsic-backed`
- `contextlib`: `intrinsic-partial`

### P2 queue (Phase 4: import/data/network long tail)
- `pathlib`: `intrinsic-partial`
- `importlib`: `probe-only`
- `importlib.util`: `intrinsic-partial`
- `importlib.machinery`: `intrinsic-backed`
- `pkgutil`: `python-only`
- `glob`: `python-only`
- `shutil`: `python-only`
- `py_compile`: `python-only`
- `compileall`: `python-only`
- `json`: `probe-only`
- `csv`: `python-only`
- `pickle`: `python-only`
- `enum`: `python-only`
- `ipaddress`: `python-only`
- `encodings`: `python-only`
- `ssl`: `not-audited`
- `subprocess`: `not-audited`
- `concurrent.futures`: `intrinsic-partial`
- `http.client`: `probe-only`
- `http.server`: `probe-only`

## Audit (Generated)
### Intrinsic-backed modules (lowering complete)
- `__future__`
- `_abc`
- `_collections_abc`
- `_intrinsics`
- `abc`
- `builtins`
- `codecs`
- `collections`
- `collections.abc`
- `copy`
- `copyreg`
- `dataclasses`
- `errno`
- `gc`
- `heapq`
- `hmac`
- `importlib.machinery`
- `io`
- `itertools`
- `keyword`
- `linecache`
- `logging`
- `molt.stdlib`
- `molt_db`
- `multiprocessing`
- `multiprocessing.spawn`
- `operator`
- `os`
- `reprlib`
- `select`
- `selectors`
- `shlex`
- `struct`
- `sys`
- `threading`
- `time`
- `traceback`
- `types`
- `typing`
- `weakref`

### Intrinsic-backed modules (partial lowering pending)
- `_asyncio`
- `asyncio`
- `concurrent.futures`
- `contextlib`
- `decimal`
- `email.message`
- `gettext`
- `hashlib`
- `importlib.metadata`
- `importlib.resources`
- `importlib.util`
- `inspect`
- `locale`
- `math`
- `pathlib`
- `re`
- `runpy`
- `socket`
- `zipfile`

### Probe-only modules (thin wrappers + policy gate only)
- `_weakrefset`
- `base64`
- `bisect`
- `contextvars`
- `fnmatch`
- `http.client`
- `http.server`
- `importlib`
- `json`
- `pprint`
- `random`
- `socketserver`
- `string`
- `tempfile`
- `unittest`
- `warnings`

### Python-only modules (intrinsic missing)
- `_bz2`
- `_weakref`
- `ast`
- `compileall`
- `concurrent`
- `csv`
- `ctypes`
- `doctest`
- `encodings`
- `encodings.aliases`
- `enum`
- `functools`
- `glob`
- `ipaddress`
- `pickle`
- `pkgutil`
- `py_compile`
- `shutil`
- `signal`
- `stat`
- `test`
- `test.import_helper`
- `test.list_tests`
- `test.os_helper`
- `test.seq_tests`
- `test.support`
- `test.tokenizedata`
- `test.tokenizedata.badsyntax_3131`
- `test.tokenizedata.badsyntax_pep3120`
- `test.warnings_helper`
- `textwrap`
- `urllib`
- `urllib.parse`
- `uuid`
- `zipimport`

## Core Lane Gate
- Required lane: `tests/differential/core/TESTS.txt` (import closure).
- Gate rule: core-lane imports must be `intrinsic-backed` only (no `intrinsic-partial`, `probe-only`, or `python-only`).
- Enforced by: `python3 tools/check_core_lane_lowering.py`.

## Bootstrap Gate
- Required modules: `__future__`, `_abc`, `_collections_abc`, `_weakrefset`, `abc`, `collections.abc`, `copy`, `copyreg`, `dataclasses`, `keyword`, `linecache`, `re`, `reprlib`, `types`, `typing`, `warnings`, `weakref`
- Gate rule: bootstrap modules must not be `python-only`.

## Critical Strict-Import Gate
- Optional strict mode: `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`.
- Critical roots: `socket`, `threading`, `asyncio`, `pathlib`, `time`, `traceback`, `sys`, `os`
- Gate rule: for each listed root currently `intrinsic-backed`, every transitive stdlib import in its closure must also be `intrinsic-backed`.
- Strict root rule: no optional intrinsic loaders and no try/except import fallback paths (applies to all listed roots, including `intrinsic-partial`).

## Intrinsic-Backed Fallback Gate
- Global rule: every `intrinsic-backed` module must avoid optional intrinsic loaders and try/except import fallback paths.
- Enforced by: `python3 tools/check_stdlib_intrinsics.py` (default mode).

## TODO
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): replace Python-only stdlib modules with Rust intrinsics and remove Python implementations; see the audit lists above.
