# Stdlib Intrinsics Backing Tracker
**Spec ID:** 0015-IB
**Status:** Draft (tracking)
**Owner:** stdlib + runtime + compiler

Tracks stdlib modules that are currently implemented in Python without runtime intrinsics.
This is a worklist for replacing pure-Python implementations with Rust-backed intrinsics.

| Module | Status | Notes |
| --- | --- | --- |
| `__future__` | Python-only | Source: `src/molt/stdlib/__future__.py` |
| `_abc` | Python-only | Source: `src/molt/stdlib/_abc.py` |
| `_asyncio` | Python-only | Source: `src/molt/stdlib/_asyncio.py` |
| `_bz2` | Python-only | Source: `src/molt/stdlib/_bz2.py` |
| `_collections_abc` | Python-only | Source: `src/molt/stdlib/_collections_abc.py` |
| `_weakref` | Python-only | Source: `src/molt/stdlib/_weakref.py` |
| `_weakrefset` | Python-only | Source: `src/molt/stdlib/_weakrefset.py` |
| `abc` | Python-only | Source: `src/molt/stdlib/abc.py` |
| `ast` | Python-only | Source: `src/molt/stdlib/ast.py` |
| `base64` | Python-only | Source: `src/molt/stdlib/base64.py` |
| `bisect` | Python-only | Source: `src/molt/stdlib/bisect.py` |
| `collections.abc` | Python-only | Source: `src/molt/stdlib/collections/abc.py` |
| `compileall` | Python-only | Source: `src/molt/stdlib/compileall.py` |
| `concurrent` | Python-only | Source: `src/molt/stdlib/concurrent/__init__.py` |
| `contextlib` | Python-only | Source: `src/molt/stdlib/contextlib.py` |
| `contextvars` | Python-only | Source: `src/molt/stdlib/contextvars.py` |
| `copyreg` | Python-only | Source: `src/molt/stdlib/copyreg.py` |
| `csv` | Python-only | Source: `src/molt/stdlib/csv.py` |
| `ctypes` | Python-only | Source: `src/molt/stdlib/ctypes.py` |
| `doctest` | Python-only | Source: `src/molt/stdlib/doctest.py` |
| `fnmatch` | Python-only | Source: `src/molt/stdlib/fnmatch.py` |
| `functools` | Python-only | Source: `src/molt/stdlib/functools.py` |
| `gc` | Python-only | Source: `src/molt/stdlib/gc.py` |
| `gettext` | Python-only | Source: `src/molt/stdlib/gettext.py` |
| `glob` | Python-only | Source: `src/molt/stdlib/glob.py` |
| `importlib` | Python-only | Source: `src/molt/stdlib/importlib/__init__.py` |
| `importlib.machinery` | Python-only | Source: `src/molt/stdlib/importlib/machinery.py` |
| `importlib.metadata` | Python-only | Source: `src/molt/stdlib/importlib/metadata.py` |
| `importlib.resources` | Python-only | Source: `src/molt/stdlib/importlib/resources/__init__.py` |
| `importlib.util` | Python-only | Source: `src/molt/stdlib/importlib/util.py` |
| `io` | Python-only | Source: `src/molt/stdlib/io.py` |
| `ipaddress` | Python-only | Source: `src/molt/stdlib/ipaddress.py` |
| `itertools` | Python-only | Source: `src/molt/stdlib/itertools.py` |
| `json` | Python-only | Source: `src/molt/stdlib/json.py` |
| `keyword` | Python-only | Source: `src/molt/stdlib/keyword.py` |
| `linecache` | Python-only | Source: `src/molt/stdlib/linecache.py` |
| `locale` | Python-only | Source: `src/molt/stdlib/locale.py` |
| `operator` | Python-only | Source: `src/molt/stdlib/operator.py` |
| `pathlib` | Python-only | Source: `src/molt/stdlib/pathlib.py` |
| `pickle` | Python-only | Source: `src/molt/stdlib/pickle.py` |
| `pkgutil` | Python-only | Source: `src/molt/stdlib/pkgutil.py` |
| `pprint` | Python-only | Source: `src/molt/stdlib/pprint.py` |
| `py_compile` | Python-only | Source: `src/molt/stdlib/py_compile.py` |
| `random` | Python-only | Source: `src/molt/stdlib/random.py` |
| `re` | Python-only | Source: `src/molt/stdlib/re.py` |
| `reprlib` | Python-only | Source: `src/molt/stdlib/reprlib.py` |
| `shlex` | Python-only | Source: `src/molt/stdlib/shlex.py` |
| `shutil` | Python-only | Source: `src/molt/stdlib/shutil.py` |
| `signal` | Python-only | Source: `src/molt/stdlib/signal.py` |
| `stat` | Python-only | Source: `src/molt/stdlib/stat.py` |
| `string` | Python-only | Source: `src/molt/stdlib/string.py` |
| `tempfile` | Python-only | Source: `src/molt/stdlib/tempfile.py` |
| `textwrap` | Python-only | Source: `src/molt/stdlib/textwrap.py` |
| `types` | Python-only | Source: `src/molt/stdlib/types.py` |
| `typing` | Python-only | Source: `src/molt/stdlib/typing.py` |
| `unittest` | Python-only | Source: `src/molt/stdlib/unittest.py` |
| `urllib` | Python-only | Source: `src/molt/stdlib/urllib/__init__.py` |
| `urllib.parse` | Python-only | Source: `src/molt/stdlib/urllib/parse.py` |
| `uuid` | Python-only | Source: `src/molt/stdlib/uuid.py` |
| `zipimport` | Python-only | Source: `src/molt/stdlib/zipimport.py` |
