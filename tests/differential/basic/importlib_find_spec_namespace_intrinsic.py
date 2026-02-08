"""Purpose: validate intrinsic-backed importlib.util namespace package resolution."""

import importlib.util
import os
import sys
import tempfile


root = tempfile.mkdtemp(prefix="molt_ns_find_spec_")
left_root = os.path.join(root, "left")
right_root = os.path.join(root, "right")
left_ns = os.path.join(left_root, "nspkg")
right_ns = os.path.join(right_root, "nspkg")
os.makedirs(left_ns, exist_ok=True)
os.makedirs(right_ns, exist_ok=True)
with open(os.path.join(right_ns, "mod.py"), "w", encoding="utf-8") as handle:
    handle.write("value = 1\n")

orig_path = list(sys.path)
orig_meta_path = list(sys.meta_path)
orig_path_hooks = list(sys.path_hooks)
orig_cache = dict(sys.path_importer_cache)
orig_modules = {name: sys.modules.get(name) for name in ("nspkg", "nspkg.mod")}
spec_cache = getattr(importlib.util, "_SPEC_CACHE", None)
if isinstance(spec_cache, dict):
    spec_cache.clear()

try:
    sys.path[:] = [left_root, right_root]
    spec_pkg = importlib.util.find_spec("nspkg")
    spec_mod = importlib.util.find_spec("nspkg.mod")
finally:
    sys.path[:] = orig_path
    sys.meta_path[:] = orig_meta_path
    sys.path_hooks[:] = orig_path_hooks
    sys.path_importer_cache.clear()
    sys.path_importer_cache.update(orig_cache)
    for name, previous in orig_modules.items():
        if previous is None:
            sys.modules.pop(name, None)
        else:
            sys.modules[name] = previous
    if isinstance(spec_cache, dict):
        spec_cache.clear()

locations = (
    set(os.path.normpath(path) for path in (spec_pkg.submodule_search_locations or []))
    if spec_pkg is not None
    else set()
)
left_norm = os.path.normpath(left_ns)
right_norm = os.path.normpath(right_ns)
mod_origin = os.path.normpath(spec_mod.origin) if spec_mod and spec_mod.origin else ""

print(spec_pkg is not None)
print(spec_pkg is not None and spec_pkg.loader is None and bool(spec_pkg.submodule_search_locations))
print(left_norm in locations and right_norm in locations)
print(spec_mod is not None)
print(bool(mod_origin) and mod_origin.endswith(os.path.normpath(os.path.join("nspkg", "mod.py"))))
print(spec_mod is not None and spec_mod.loader is not None)
