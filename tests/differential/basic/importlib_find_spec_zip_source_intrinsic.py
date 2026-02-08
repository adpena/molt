"""Purpose: validate intrinsic-backed zip source loader find_spec + exec_module."""

import importlib.util
import os
import sys
import tempfile
import zipfile


root = tempfile.mkdtemp(prefix="molt_zip_find_spec_")
archive = os.path.join(root, "mods.zip")
with zipfile.ZipFile(archive, "w") as zf:
    zf.writestr("zipmod.py", "value = 11\n")
    zf.writestr("zpkg/__init__.py", "flag = 7\n")

orig_path = list(sys.path)
orig_modules = {name: sys.modules.get(name) for name in ("zipmod", "zpkg")}
spec_cache = getattr(importlib.util, "_SPEC_CACHE", None)
if isinstance(spec_cache, dict):
    spec_cache.clear()

try:
    sys.path[:] = [archive]
    mod_spec = importlib.util.find_spec("zipmod")
    pkg_spec = importlib.util.find_spec("zpkg")
    mod = None
    pkg = None
    if mod_spec is not None and mod_spec.loader is not None:
        mod = importlib.util.module_from_spec(mod_spec)
        mod_spec.loader.exec_module(mod)
    if pkg_spec is not None and pkg_spec.loader is not None:
        pkg = importlib.util.module_from_spec(pkg_spec)
        pkg_spec.loader.exec_module(pkg)
finally:
    sys.path[:] = orig_path
    for name, previous in orig_modules.items():
        if previous is None:
            sys.modules.pop(name, None)
        else:
            sys.modules[name] = previous
    if isinstance(spec_cache, dict):
        spec_cache.clear()

print(mod_spec is not None and mod_spec.loader is not None)
print(pkg_spec is not None and pkg_spec.loader is not None)
print(
    mod_spec is not None
    and mod_spec.origin is not None
    and mod_spec.origin.endswith("mods.zip/zipmod.py")
)
print(
    pkg_spec is not None
    and pkg_spec.origin is not None
    and pkg_spec.origin.endswith("mods.zip/zpkg/__init__.py")
)
print(mod is not None and getattr(mod, "value", None) == 11)
print(pkg is not None and getattr(pkg, "flag", None) == 7)
