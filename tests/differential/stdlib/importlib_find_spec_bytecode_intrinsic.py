"""Purpose: validate intrinsic-backed importlib.util sourceless bytecode find_spec."""

import importlib.util
import os
import sys
import tempfile


root = tempfile.mkdtemp(prefix="molt_bytecode_find_spec_")
mod_pyc = os.path.join(root, "bcmod.pyc")
with open(mod_pyc, "wb") as handle:
    handle.write(b"bytecode")

pkg_dir = os.path.join(root, "bcpkg")
os.makedirs(pkg_dir, exist_ok=True)
pkg_pyc = os.path.join(pkg_dir, "__init__.pyc")
with open(pkg_pyc, "wb") as handle:
    handle.write(b"bytecode")

orig_path = list(sys.path)
orig_modules = {name: sys.modules.get(name) for name in ("bcmod", "bcpkg")}
spec_cache = getattr(importlib.util, "_SPEC_CACHE", None)
if isinstance(spec_cache, dict):
    spec_cache.clear()

try:
    sys.path[:] = [root]
    mod_spec = importlib.util.find_spec("bcmod")
    pkg_spec = importlib.util.find_spec("bcpkg")
finally:
    sys.path[:] = orig_path
    for name, previous in orig_modules.items():
        if previous is None:
            sys.modules.pop(name, None)
        else:
            sys.modules[name] = previous
    if isinstance(spec_cache, dict):
        spec_cache.clear()

mod_origin = os.path.normpath(mod_spec.origin) if mod_spec and mod_spec.origin else ""
pkg_origin = os.path.normpath(pkg_spec.origin) if pkg_spec and pkg_spec.origin else ""
print(mod_spec is not None and mod_spec.loader is not None)
print(bool(mod_origin) and mod_origin.endswith(os.path.normpath("bcmod.pyc")))
print(pkg_spec is not None and pkg_spec.loader is not None and bool(pkg_spec.submodule_search_locations))
print(bool(pkg_origin) and pkg_origin.endswith(os.path.normpath(os.path.join("bcpkg", "__init__.pyc"))))
