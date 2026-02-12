"""Purpose: validate intrinsic-backed path_importer_cache semantics in find_spec."""

import importlib.machinery
import importlib.util
import sys


class _Loader:
    def create_module(self, spec):
        return None

    def exec_module(self, module):
        return None


class _PathFinder:
    def find_spec(self, fullname, target=None):
        if fullname != "molt_cache_target":
            return None
        spec = importlib.machinery.ModuleSpec(
            fullname,
            loader=_Loader(),
            origin="cache://origin",
            is_package=False,
        )
        spec.has_location = False
        return spec


def _path_hook(path):
    _path_hook.calls += 1
    if path == "molt://cache-hook":
        return _PathFinder()
    raise ImportError(path)


_path_hook.calls = 0

orig_path_hooks = list(sys.path_hooks)
orig_path = list(sys.path)
orig_cache = dict(sys.path_importer_cache)
spec_cache = getattr(importlib.util, "_SPEC_CACHE", None)
if isinstance(spec_cache, dict):
    spec_cache.clear()

try:
    sys.path_hooks[:] = [_path_hook]
    sys.path[:] = ["molt://cache-hook"]
    sys.path_importer_cache.clear()
    spec1 = importlib.util.find_spec("molt_cache_target")
    if isinstance(spec_cache, dict):
        spec_cache.clear()
    spec2 = importlib.util.find_spec("molt_cache_target")
    cache_has_entry = "molt://cache-hook" in sys.path_importer_cache
finally:
    sys.path_hooks[:] = orig_path_hooks
    sys.path[:] = orig_path
    sys.path_importer_cache.clear()
    sys.path_importer_cache.update(orig_cache)
    if isinstance(spec_cache, dict):
        spec_cache.clear()

print(spec1 is not None)
print(spec2 is not None)
print(_path_hook.calls)
print(spec1.origin if spec1 else None)
print(cache_has_entry)
