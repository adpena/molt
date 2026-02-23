"""Purpose: validate find_spec parent resolution when parent is not pre-imported."""

import importlib.util
import os
import sys
import tempfile


spec_cache = getattr(importlib.util, "_SPEC_CACHE", None)
if isinstance(spec_cache, dict):
    spec_cache.clear()

with tempfile.TemporaryDirectory(prefix="molt_find_spec_parent_") as root:
    pkg_dir = os.path.join(root, "parentpkg")
    os.makedirs(pkg_dir, exist_ok=True)
    with open(os.path.join(pkg_dir, "__init__.py"), "w", encoding="utf-8") as handle:
        handle.write("value = 1\n")
    with open(os.path.join(pkg_dir, "child.py"), "w", encoding="utf-8") as handle:
        handle.write("value = 2\n")

    orig_path = list(sys.path)
    orig_modules = {
        name: sys.modules.get(name) for name in ("parentpkg", "parentpkg.child")
    }
    before_parent_in_modules = False
    child_spec = None
    parent_spec = None

    try:
        sys.path[:] = [root]
        sys.modules.pop("parentpkg", None)
        sys.modules.pop("parentpkg.child", None)
        before_parent_in_modules = "parentpkg" in sys.modules

        child_spec = importlib.util.find_spec("parentpkg.child")
        parent_spec = importlib.util.find_spec("parentpkg")
    finally:
        sys.path[:] = orig_path
        for name, previous in orig_modules.items():
            if previous is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = previous
        if isinstance(spec_cache, dict):
            spec_cache.clear()

child_origin = os.path.normpath(child_spec.origin) if child_spec and child_spec.origin else ""
parent_origin = (
    os.path.normpath(parent_spec.origin) if parent_spec and parent_spec.origin else ""
)

print(before_parent_in_modules)
print(child_spec is not None)
print(
    bool(child_origin)
    and child_origin.endswith(os.path.normpath(os.path.join("parentpkg", "child.py")))
)
print(parent_spec is not None)
print(
    bool(parent_origin)
    and parent_origin.endswith(os.path.normpath(os.path.join("parentpkg", "__init__.py")))
)
print(parent_spec is not None and bool(parent_spec.submodule_search_locations))
