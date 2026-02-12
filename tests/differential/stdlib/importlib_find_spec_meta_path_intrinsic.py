"""Purpose: validate intrinsic-backed importlib.util.find_spec meta_path execution."""

import importlib.machinery
import importlib.util
import sys


class _MetaFinder:
    def find_spec(self, fullname, path=None, target=None):
        if fullname != "molt_meta_target":
            return None
        spec = importlib.machinery.ModuleSpec(
            fullname,
            loader=None,
            origin="meta://finder",
            is_package=False,
        )
        spec.has_location = False
        return spec


orig_meta_path = list(sys.meta_path)
try:
    sys.meta_path[:] = [_MetaFinder()]
    spec = importlib.util.find_spec("molt_meta_target")
    missing = importlib.util.find_spec("molt_meta_missing")
finally:
    sys.meta_path[:] = orig_meta_path

print(spec is not None)
print(spec.origin if spec else None)
print(spec.has_location if spec else None)
print(missing is None)
