"""Purpose: `import importlib` neither loads nor binds importlib.util.

CPython 3.12-3.15 `importlib/__init__.py` imports only `_bootstrap` and
`_bootstrap_external`. `importlib.util` loads on its first explicit import, and
the package `__all__` names no submodule.
"""

import sys

import importlib

print("util loaded", "importlib.util" in sys.modules)
print("util bound", hasattr(importlib, "util"))
print("util exported", "util" in importlib.__all__)

from importlib import *  # noqa: E402,F403

_CANDIDATES = (
    "import_module",
    "invalidate_caches",
    "reload",
    "machinery",
    "util",
    "resources",
    "metadata",
)
print("star", sorted(name for name in _CANDIDATES if name in globals()))

from importlib import util  # noqa: E402

print("from-import", util.__name__, importlib.util is util)
print("published", sys.modules["importlib.util"] is util)
print("resolve_name", util.resolve_name(".child", "pkg"))
