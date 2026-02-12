"""Purpose: validate sourceless __pycache__ package shim execution intrinsic lane."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_sourceless_pkg_cache_")
pkg_dir = os.path.join(root, "pkgcache")
cache_dir = os.path.join(pkg_dir, "__pycache__")
os.makedirs(cache_dir, exist_ok=True)
pyc_path = os.path.join(cache_dir, "__init__.cpython-312.pyc")
with open(pyc_path, "wb") as handle:
    handle.write(b"")
with open(os.path.join(pkg_dir, "__init__.molt.py"), "w", encoding="utf-8") as handle:
    handle.write("value = 91\n")

loader = importlib.machinery.SourcelessFileLoader("pkgcache_exec", pyc_path)
spec = importlib.util.spec_from_file_location(
    "pkgcache_exec", pyc_path, loader=loader, submodule_search_locations=[pkg_dir]
)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
is_pkg = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = getattr(module, "value", None) == 91
        is_pkg = isinstance(getattr(module, "__path__", None), list)
except BaseException as exc:
    error_name = exc.__class__.__name__

print(
    loaded
    or error_name
    in {"ImportError", "PermissionError", "OSError", "EOFError", "RuntimeError"}
)
print((not loaded) or is_pkg)
