# Stdlib Intrinsics Audit
**Spec ID:** 0016
**Status:** Draft (enforcement + audit)
**Owner:** stdlib + runtime

## Policy
- Compiled binaries must not execute Python stdlib implementations.
- Every stdlib module must be backed by Rust intrinsics (Python files are allowed only as thin, intrinsic-forwarding wrappers).
- Modules without intrinsic usage are forbidden in compiled builds and must raise immediately until fully lowered.

## Audit (Generated)
### Intrinsic-backed modules
- `_intrinsics`
- `asyncio`
- `builtins`
- `codecs`
- `collections`
- `concurrent.futures`
- `decimal`
- `email.message`
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
- `re`
- `runpy`
- `select`
- `selectors`
- `shlex`
- `socket`
- `struct`
- `sys`
- `threading`
- `time`
- `types`
- `weakref`
- `zipfile`

### Probe-only modules (thin wrappers + policy gate only)
- `__future__`
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
- `keyword`
- `linecache`
- `molt.stdlib`
- `pprint`
- `random`
- `reprlib`
- `socketserver`
- `string`
- `tempfile`
- `traceback`
- `typing`
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
- `itertools`
- `locale`
- `operator`
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

## Bootstrap Gate
- Required modules: `__future__`, `_abc`, `_collections_abc`, `_weakrefset`, `abc`, `collections.abc`, `copy`, `copyreg`, `dataclasses`, `keyword`, `linecache`, `re`, `reprlib`, `types`, `typing`, `warnings`, `weakref`
- Gate rule: bootstrap modules must not be `python-only`.

## TODO
- TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:missing): replace Python-only stdlib modules with Rust intrinsics and remove Python implementations; see the audit lists above.
