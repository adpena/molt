# Stdlib Intrinsics Audit
**Spec ID:** 0016
**Status:** Draft (enforcement + audit)
**Owner:** stdlib + runtime

## Policy
- Compiled binaries must not execute Python stdlib implementations.
- Every stdlib module must be backed by Rust intrinsics (Python files are allowed only as thin, intrinsic-forwarding wrappers).
- Modules without intrinsic usage are forbidden in compiled builds and must raise immediately until fully lowered.

## Audit (2026-02-05)
### Intrinsic-backed modules
- `_intrinsics`
- `asyncio`
- `builtins`
- `codecs`
- `collections`
- `concurrent.futures`
- `decimal`
- `errno`
- `hashlib`
- `heapq`
- `hmac`
- `inspect`
- `io`
- `logging`
- `math`
- `molt_db`
- `multiprocessing`
- `multiprocessing.spawn`
- `os`
- `select`
- `selectors`
- `socket`
- `struct`
- `sys`
- `threading`
- `time`
- `zipfile`

### Python-only modules (intrinsic missing)
- `__future__`
- `_abc`
- `_asyncio`
- `_bz2`
- `_collections_abc`
- `_weakref`
- `_weakrefset`
- `abc`
- `ast`
- `base64`
- `bisect`
- `collections.abc`
- `compileall`
- `concurrent`
- `contextlib`
- `contextvars`
- `copy`
- `copyreg`
- `csv`
- `ctypes`
- `dataclasses`
- `doctest`
- `encodings`
- `encodings.aliases`
- `enum`
- `fnmatch`
- `functools`
- `gc`
- `gettext`
- `glob`
- `importlib`
- `importlib.machinery`
- `importlib.metadata`
- `importlib.resources`
- `importlib.util`
- `ipaddress`
- `itertools`
- `json`
- `keyword`
- `linecache`
- `locale`
- `molt.stdlib`
- `operator`
- `pathlib`
- `pickle`
- `pkgutil`
- `pprint`
- `py_compile`
- `random`
- `re`
- `reprlib`
- `shlex`
- `shutil`
- `signal`
- `stat`
- `string`
- `tempfile`
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
- `traceback`
- `types`
- `typing`
- `unittest`
- `urllib`
- `urllib.parse`
- `uuid`
- `warnings`
- `weakref`
- `zipimport`

## TODO
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): replace Python-only stdlib modules with Rust intrinsics and remove Python implementations; see the audit lists above.
