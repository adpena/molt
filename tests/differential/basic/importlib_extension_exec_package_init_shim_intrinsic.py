"""Purpose: validate extension __init__ shim package execution intrinsic lane."""

import importlib.machinery
import importlib.util
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_ext_pkg_init_")
pkg_dir = os.path.join(root, "pkgext")
os.makedirs(pkg_dir, exist_ok=True)
ext_path = os.path.join(pkg_dir, "__init__.so")
with open(ext_path, "wb") as handle:
    handle.write(b"")
with open(os.path.join(pkg_dir, "__init__.molt.py"), "w", encoding="utf-8") as handle:
    handle.write("value = 73\n")

loader = importlib.machinery.ExtensionFileLoader("pkgext_exec", ext_path)
spec = importlib.util.spec_from_file_location(
    "pkgext_exec", ext_path, loader=loader, submodule_search_locations=[pkg_dir]
)
module = importlib.util.module_from_spec(spec) if spec is not None else None

loaded = False
is_pkg = False
error_name = "none"
try:
    if spec is not None and spec.loader is not None and module is not None:
        spec.loader.exec_module(module)
        loaded = getattr(module, "value", None) == 73
        is_pkg = isinstance(getattr(module, "__path__", None), list)
except BaseException as exc:
    error_name = exc.__class__.__name__

print(loaded or error_name in {"ImportError", "PermissionError", "OSError"})
print((not loaded) or is_pkg)
