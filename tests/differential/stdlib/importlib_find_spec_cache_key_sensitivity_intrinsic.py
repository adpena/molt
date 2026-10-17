"""Purpose: validate find_spec observes current public importer state."""

import importlib.machinery
import importlib.util
import os.path
import sys
import types


META_TARGET = "molt_find_spec_meta_sig_target"
PATH_TARGET = "molt_find_spec_path_sig_target"
CACHE_TARGET = "molt_find_spec_cache_sig_target"
PATH_ENTRY = "molt://sig-path"
CACHE_ENTRY = "molt://sig-cache"
PUBLISHED_TARGET = "molt_find_spec_published_target"


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


class _PublishingFinder:
    def __init__(self, module, provisional):
        self.module = module
        self.provisional = provisional
        self.calls = 0

    def find_spec(self, fullname, path=None, target=None):
        if fullname != PUBLISHED_TARGET:
            return None
        self.calls += 1
        sys.modules[fullname] = self.module
        return self.provisional


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
missing = object()
orig_published = sys.modules.get(PUBLISHED_TARGET, missing)

# Only the final selected spec may trigger native artifact admission. The
# unused provisional artifact is never created or loaded.
published_spec = importlib.machinery.ModuleSpec(
    PUBLISHED_TARGET, _Loader(), origin="published://ordinary"
)
published_module = types.ModuleType(PUBLISHED_TARGET)
published_module.__spec__ = published_spec
unused_extension = os.path.abspath("molt_hf297_unused_provisional.so")
provisional_spec = importlib.machinery.ModuleSpec(
    PUBLISHED_TARGET,
    importlib.machinery.ExtensionFileLoader(PUBLISHED_TARGET, unused_extension),
    origin=unused_extension,
)
publishing_finder = _PublishingFinder(published_module, provisional_spec)

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
    # Each call must use the current meta-path finder.
    sys.meta_path[:] = [_MetaFinder(META_TARGET, "meta://first")]
    spec_meta_1 = importlib.util.find_spec(META_TARGET)
    sys.meta_path[:] = [_MetaFinder(META_TARGET, "meta://second")]
    spec_meta_2 = importlib.util.find_spec(META_TARGET)

    # Clearing the public importer cache must select the current path hook.
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

    # Each call must use the current public path_importer_cache value.
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

    # The callback publishes an ordinary spec before returning an unused native
    # provisional result; CPython selects the newly published public spec.
    sys.modules.pop(PUBLISHED_TARGET, None)
    sys.meta_path[:] = [publishing_finder]
    selected_spec = importlib.util.find_spec(PUBLISHED_TARGET)
    published_cache_identity = sys.modules[PUBLISHED_TARGET] is published_module
    assert selected_spec is published_spec
    assert publishing_finder.calls == 1
    assert published_cache_identity
finally:
    sys.meta_path[:] = orig_meta_path
    sys.path_hooks[:] = orig_path_hooks
    sys.path[:] = orig_path
    sys.path_importer_cache.clear()
    sys.path_importer_cache.update(orig_cache)
    if orig_published is missing:
        sys.modules.pop(PUBLISHED_TARGET, None)
    else:
        sys.modules[PUBLISHED_TARGET] = orig_published

print(spec_meta_1.origin if spec_meta_1 else None)
print(spec_meta_2.origin if spec_meta_2 else None)
print(spec_path_1.origin if spec_path_1 else None)
print(spec_path_2.origin if spec_path_2 else None)
print(hook_a_calls)
print(hook_b_calls)
print(spec_cache_1.origin if spec_cache_1 else None)
print(spec_cache_2.origin if spec_cache_2 else None)
print(hook_cache_calls)
print("published-selection", selected_spec is published_spec, publishing_finder.calls)
print("published-cache", published_cache_identity)
