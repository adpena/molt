"""Purpose: validate intrinsic-backed importlib.util.find_spec path_hooks execution."""

import importlib.machinery
import importlib.util
import sys


class _PathFinder:
    def find_spec(self, fullname, target=None):
        if fullname != "molt_path_target":
            return None
        spec = importlib.machinery.ModuleSpec(
            fullname,
            loader=None,
            origin="pathhook://finder",
            is_package=False,
        )
        spec.has_location = False
        return spec


def _path_hook(path):
    if path == "molt://hook":
        return _PathFinder()
    raise ImportError(path)


orig_meta_path = list(sys.meta_path)
orig_path_hooks = list(sys.path_hooks)
orig_path = list(sys.path)
orig_cache = dict(sys.path_importer_cache)
try:
    sys.meta_path[:] = []
    sys.path_hooks[:] = [_path_hook]
    sys.path[:] = ["molt://hook"]
    sys.path_importer_cache.clear()
    spec = importlib.util.find_spec("molt_path_target")
    missing = importlib.util.find_spec("molt_path_missing")
finally:
    sys.meta_path[:] = orig_meta_path
    sys.path_hooks[:] = orig_path_hooks
    sys.path[:] = orig_path
    sys.path_importer_cache.clear()
    sys.path_importer_cache.update(orig_cache)

print(spec is not None)
print(spec.origin if spec else None)
print(spec.has_location if spec else None)
print(missing is None)
