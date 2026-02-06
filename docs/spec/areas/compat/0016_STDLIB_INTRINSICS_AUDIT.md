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
- `intrinsic-backed`: `21`
- `intrinsic-partial`: `17`
- `probe-only`: `30`
- `python-only`: `42`

## Priority Lowering Queue (Generated)
### P0 queue (Phase 2: concurrency substrate)
- `socket`: `intrinsic-partial`
- `select`: `intrinsic-partial`
- `selectors`: `intrinsic-backed`
- `threading`: `intrinsic-partial`
- `asyncio`: `intrinsic-partial`

### P1 queue (Phase 3: core-adjacent stdlib)
- `builtins`: `intrinsic-backed`
- `types`: `intrinsic-partial`
- `weakref`: `intrinsic-partial`
- `math`: `intrinsic-partial`
- `re`: `intrinsic-partial`
- `struct`: `intrinsic-partial`
- `time`: `intrinsic-partial`
- `inspect`: `intrinsic-partial`
- `functools`: `python-only`
- `itertools`: `intrinsic-backed`
- `operator`: `intrinsic-backed`
- `contextlib`: `python-only`

### P2 queue (Phase 4: import/data/network long tail)
- `pathlib`: `python-only`
- `importlib`: `probe-only`
- `importlib.util`: `probe-only`
- `importlib.machinery`: `probe-only`
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
- `_intrinsics`
- `builtins`
- `codecs`
- `collections`
- `errno`
- `heapq`
- `hmac`
- `io`
- `itertools`
- `keyword`
- `logging`
- `molt_db`
- `multiprocessing`
- `multiprocessing.spawn`
- `operator`
- `os`
- `selectors`
- `shlex`
- `sys`
- `typing`

### Intrinsic-backed modules (partial lowering pending)
- `asyncio`
- `concurrent.futures`
- `decimal`
- `email.message`
- `hashlib`
- `inspect`
- `math`
- `re`
- `runpy`
- `select`
- `socket`
- `struct`
- `threading`
- `time`
- `types`
- `weakref`
- `zipfile`

### Probe-only modules (thin wrappers + policy gate only)
- `_abc`
- `_collections_abc`
- `_weakrefset`
- `abc`
- `base64`
- `bisect`
- `collections.abc`
- `contextvars`
- `copy`
- `copyreg`
- `dataclasses`
- `fnmatch`
- `gc`
- `http.client`
- `http.server`
- `importlib`
- `importlib.machinery`
- `importlib.util`
- `json`
- `linecache`
- `molt.stdlib`
- `pprint`
- `random`
- `reprlib`
- `socketserver`
- `string`
- `tempfile`
- `traceback`
- `unittest`
- `warnings`

### Python-only modules (intrinsic missing)
- `_asyncio`
- `_bz2`
- `_weakref`
- `ast`
- `compileall`
- `concurrent`
- `contextlib`
- `csv`
- `ctypes`
- `doctest`
- `encodings`
- `encodings.aliases`
- `enum`
- `functools`
- `gettext`
- `glob`
- `importlib.metadata`
- `importlib.resources`
- `ipaddress`
- `locale`
- `pathlib`
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

## TODO
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): replace Python-only stdlib modules with Rust intrinsics and remove Python implementations; see the audit lists above.
