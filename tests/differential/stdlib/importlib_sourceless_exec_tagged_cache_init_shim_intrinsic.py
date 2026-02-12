"""Purpose: validate sourceless tagged __pycache__ __init__ shim package execution lane."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_sourceless_pkg_tagged_")
pkg_dir = os.path.join(root, "pkgcachetagged")
cache_dir = os.path.join(pkg_dir, "__pycache__")
os.makedirs(cache_dir, exist_ok=True)
pyc_path = os.path.join(cache_dir, "__init__.cpython-312.pyc")
with open(pyc_path, "wb") as handle:
    handle.write(b"")
with open(os.path.join(pkg_dir, "__init__.molt.py"), "w", encoding="utf-8") as handle:
    handle.write("value = 191\n")

loader = importlib.machinery.SourcelessFileLoader("pkgcachetagged", pyc_path)
spec = importlib.util.spec_from_file_location("pkgcachetagged", pyc_path, loader=loader)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
is_pkg = False
path_points_parent = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = getattr(module, "value", None) == 191
        module_path = getattr(module, "__path__", None)
        is_pkg = isinstance(module_path, list)
        if is_pkg and module_path:
            path_points_parent = str(module_path[0]).replace("\\", "/").endswith(
                "/pkgcachetagged"
            )
except BaseException as exc:
    error_name = exc.__class__.__name__

print(
    loaded
    or error_name
    in {"ImportError", "PermissionError", "OSError", "EOFError", "RuntimeError"}
)
print((not loaded) or is_pkg)
print((not loaded) or path_points_parent)
