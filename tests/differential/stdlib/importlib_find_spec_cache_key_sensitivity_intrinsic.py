"""Purpose: validate find_spec cache-key sensitivity for runtime importer state."""

import importlib.machinery
import importlib.util
import sys


META_TARGET = "molt_find_spec_meta_sig_target"
PATH_TARGET = "molt_find_spec_path_sig_target"
CACHE_TARGET = "molt_find_spec_cache_sig_target"
PATH_ENTRY = "molt://sig-path"
CACHE_ENTRY = "molt://sig-cache"


class _Loader:
    def create_module(self, spec):
        return None

    def exec_module(self, module):
        return None


class _MetaFinder:
    def __init__(self, module_name, origin):
        self._module_name = module_name
        self._origin = origin

    def find_spec(self, fullname, path=None, target=None):
        del path, target
        if fullname != self._module_name:
            return None
        spec = importlib.machinery.ModuleSpec(
            fullname,
            loader=_Loader(),
            origin=self._origin,
            is_package=False,
        )
        spec.has_location = False
        return spec


class _PathFinder:
    def __init__(self, module_name, origin):
        self._module_name = module_name
        self._origin = origin

    def find_spec(self, fullname, target=None):
        del target
        if fullname != self._module_name:
            return None
        spec = importlib.machinery.ModuleSpec(
            fullname,
            loader=_Loader(),
            origin=self._origin,
            is_package=False,
        )
        spec.has_location = False
        return spec


def _make_path_hook(module_name, entry, origin):
    def _path_hook(path):
        _path_hook.calls += 1
        if path == entry:
            return _PathFinder(module_name, origin)
        raise ImportError(path)

    _path_hook.calls = 0
    return _path_hook


orig_meta_path = list(sys.meta_path)
orig_path_hooks = list(sys.path_hooks)
orig_path = list(sys.path)
orig_cache = dict(sys.path_importer_cache)
spec_cache = getattr(importlib.util, "_SPEC_CACHE", None)
if isinstance(spec_cache, dict):
    spec_cache.clear()

spec_meta_1 = None
spec_meta_2 = None
spec_path_1 = None
spec_path_2 = None
spec_cache_1 = None
spec_cache_2 = None
hook_a_calls = 0
hook_b_calls = 0
hook_cache_calls = 0

try:
    # Meta-path mutation should change the find_spec cache key.
    sys.meta_path[:] = [_MetaFinder(META_TARGET, "meta://first")]
    spec_meta_1 = importlib.util.find_spec(META_TARGET)
    sys.meta_path[:] = [_MetaFinder(META_TARGET, "meta://second")]
    spec_meta_2 = importlib.util.find_spec(META_TARGET)

    # path_hooks mutation should change the find_spec cache key.
    hook_a = _make_path_hook(PATH_TARGET, PATH_ENTRY, "pathhook://first")
    hook_b = _make_path_hook(PATH_TARGET, PATH_ENTRY, "pathhook://second")
    sys.meta_path[:] = [importlib.machinery.PathFinder]
    sys.path[:] = [PATH_ENTRY]
    sys.path_hooks[:] = [hook_a]
    sys.path_importer_cache.clear()
    spec_path_1 = importlib.util.find_spec(PATH_TARGET)
    sys.path_hooks[:] = [hook_b]
    sys.path_importer_cache.clear()
    spec_path_2 = importlib.util.find_spec(PATH_TARGET)
    hook_a_calls = hook_a.calls
    hook_b_calls = hook_b.calls

    # path_importer_cache value identity should change the cache key.
    hook_cache = _make_path_hook(CACHE_TARGET, CACHE_ENTRY, "cache://from-hook")
    sys.meta_path[:] = [importlib.machinery.PathFinder]
    sys.path[:] = [CACHE_ENTRY]
    sys.path_hooks[:] = [hook_cache]
    sys.path_importer_cache.clear()
    spec_cache_1 = importlib.util.find_spec(CACHE_TARGET)
    sys.path_importer_cache[CACHE_ENTRY] = _PathFinder(
        CACHE_TARGET, "cache://from-cache"
    )
    spec_cache_2 = importlib.util.find_spec(CACHE_TARGET)
    hook_cache_calls = hook_cache.calls
finally:
    sys.meta_path[:] = orig_meta_path
    sys.path_hooks[:] = orig_path_hooks
    sys.path[:] = orig_path
    sys.path_importer_cache.clear()
    sys.path_importer_cache.update(orig_cache)
    if isinstance(spec_cache, dict):
        spec_cache.clear()

print(spec_meta_1.origin if spec_meta_1 else None)
print(spec_meta_2.origin if spec_meta_2 else None)
print(spec_path_1.origin if spec_path_1 else None)
print(spec_path_2.origin if spec_path_2 else None)
print(hook_a_calls)
print(hook_b_calls)
print(spec_cache_1.origin if spec_cache_1 else None)
print(spec_cache_2.origin if spec_cache_2 else None)
print(hook_cache_calls)
