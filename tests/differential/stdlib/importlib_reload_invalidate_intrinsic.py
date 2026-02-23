# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for importlib reload/invalidate intrinsic paths."""

import importlib
import importlib.machinery
import os
import sys
import tempfile


invalidate_result = importlib.invalidate_caches()
print("importlib_invalidate_none", invalidate_result is None)
assert invalidate_result is None

module_name = "molt_importlib_reload_intrinsic_target"
with tempfile.TemporaryDirectory(prefix="molt_importlib_reload_") as root:
    module_path = os.path.join(root, module_name + ".py")
    with open(module_path, "w", encoding="utf-8") as handle:
        handle.write("VALUE = 1\n")
    sys.path.insert(0, root)
    try:
        importlib.invalidate_caches()
        module = importlib.import_module(module_name)
        initial_value_is_int = isinstance(getattr(module, "VALUE", None), int)
        print("initial_value_is_int", initial_value_is_int)
        assert initial_value_is_int

        with open(module_path, "w", encoding="utf-8") as handle:
            handle.write("VALUE = 2\n")
        importlib.invalidate_caches()
        reloaded = importlib.reload(module)
        reload_identity = reloaded is module
        reloaded_value_is_int = isinstance(getattr(reloaded, "VALUE", None), int)
        print("reload_identity", reload_identity)
        print("reloaded_value_is_int", reloaded_value_is_int)
        assert reload_identity
        assert reloaded_value_is_int
    finally:
        if root in sys.path:
            sys.path.remove(root)
        sys.modules.pop(module_name, None)

with tempfile.TemporaryDirectory(prefix="molt_importlib_filefinder_") as root:
    finder = importlib.machinery.FileFinder(
        root,
        (importlib.machinery.SourceFileLoader, importlib.machinery.SOURCE_SUFFIXES),
    )
    finder_invalidate = finder.invalidate_caches()
    print("filefinder_invalidate_none", finder_invalidate is None)
    assert finder_invalidate is None
