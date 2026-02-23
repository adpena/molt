"""Purpose: validate find_spec cache invalidation via importlib.invalidate_caches."""

import importlib
import importlib.machinery
import importlib.util
import sys


TARGET = "molt_find_spec_invalidate_target"
ENTRY = "molt://invalidate"


class _Loader:
    def create_module(self, spec):
        return None

    def exec_module(self, module):
        return None


class _PathFinder:
    def __init__(self, tag):
        self._tag = tag

    def find_spec(self, fullname, target=None):
        del target
        if fullname != TARGET:
            return None
        spec = importlib.machinery.ModuleSpec(
            fullname,
            loader=_Loader(),
            origin=f"invalidate://{self._tag}",
            is_package=False,
        )
        spec.has_location = False
        return spec


state = {"tag": "before"}


def _path_hook(path):
    if path == ENTRY:
        _path_hook.calls += 1
        return _PathFinder(state["tag"])
    raise ImportError(path)


_path_hook.calls = 0

orig_meta_path = list(sys.meta_path)
orig_path_hooks = list(sys.path_hooks)
orig_path = list(sys.path)
orig_cache = dict(sys.path_importer_cache)
spec_cache = getattr(importlib.util, "_SPEC_CACHE", None)
if isinstance(spec_cache, dict):
    spec_cache.clear()

first_spec = None
second_spec = None
third_spec = None
calls_before_invalidate = 0
calls_after_invalidate = 0
invalidate_result = "not-called"

try:
    sys.meta_path[:] = orig_meta_path
    sys.path_hooks[:] = [_path_hook, *orig_path_hooks]
    sys.path[:] = [ENTRY, *(path for path in orig_path if path != ENTRY)]
    sys.path_importer_cache.clear()

    first_spec = importlib.util.find_spec(TARGET)
    state["tag"] = "after"
    second_spec = importlib.util.find_spec(TARGET)
    calls_before_invalidate = _path_hook.calls

    invalidate_result = importlib.invalidate_caches()
    third_spec = importlib.util.find_spec(TARGET)
    calls_after_invalidate = _path_hook.calls
finally:
    sys.meta_path[:] = orig_meta_path
    sys.path_hooks[:] = orig_path_hooks
    sys.path[:] = orig_path
    sys.path_importer_cache.clear()
    sys.path_importer_cache.update(orig_cache)
    if isinstance(spec_cache, dict):
        spec_cache.clear()

print(first_spec.origin if first_spec else None)
print(second_spec.origin if second_spec else None)
print(third_spec.origin if third_spec else None)
print(calls_before_invalidate)
print(calls_after_invalidate)
print(invalidate_result is None)
