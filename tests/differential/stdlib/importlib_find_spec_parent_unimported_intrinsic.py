"""Purpose: validate find_spec parent resolution when parent is not pre-imported."""

import importlib.machinery
import importlib.util
import os
import sys
import tempfile


class _ParentLoader:
    def __init__(self, path):
        self.path = path
        self.executions = 0

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        self.executions += 1
        module.__path__ = self.path
        module.marker = "parent-loader-executed"


class _ParentFinder:
    def __init__(self, loader, origin):
        self.loader = loader
        self.origin = origin

    def find_spec(self, fullname, path=None, target=None):
        if fullname != "parentpkg":
            return None
        spec = importlib.machinery.ModuleSpec(
            fullname, self.loader, origin=self.origin, is_package=True
        )
        spec.submodule_search_locations = self.loader.path
        return spec


with tempfile.TemporaryDirectory(prefix="molt_find_spec_parent_") as root:
    pkg_dir = os.path.join(root, "parentpkg")
    os.makedirs(pkg_dir, exist_ok=True)
    init_path = os.path.join(pkg_dir, "__init__.py")
    with open(init_path, "w", encoding="utf-8") as handle:
        handle.write("value = 1\n")
    with open(os.path.join(pkg_dir, "child.py"), "w", encoding="utf-8") as handle:
        handle.write("raise AssertionError('child spec discovery executed source')\n")

    parent_path = [pkg_dir]
    parent_loader = _ParentLoader(parent_path)
    parent_finder = _ParentFinder(parent_loader, init_path)
    orig_path = list(sys.path)
    orig_meta_path = list(sys.meta_path)
    missing = object()
    orig_modules = {
        name: sys.modules.get(name, missing) for name in ("parentpkg", "parentpkg.child")
    }
    before_parent_in_modules = False
    child_spec = None
    parent_spec = None

    try:
        sys.path[:] = [root]
        # Execute the parent protocol without interpreting runtime-created source.
        # The parent-only finder leaves child discovery to the actual defaults.
        sys.meta_path[:] = [parent_finder, *orig_meta_path]
        sys.modules.pop("parentpkg", None)
        sys.modules.pop("parentpkg.child", None)
        before_parent_in_modules = "parentpkg" in sys.modules

        child_spec = importlib.util.find_spec("parentpkg.child")
        parent_spec = importlib.util.find_spec("parentpkg")
        parent = sys.modules["parentpkg"]
        assert parent_loader.executions == 1
        assert parent.marker == "parent-loader-executed"
        assert parent.__path__ is parent_path
        assert child_spec is not None
        assert isinstance(child_spec.loader, importlib.machinery.SourceFileLoader)
        assert "parentpkg.child" not in sys.modules
        assert not hasattr(parent, "child")
    finally:
        sys.path[:] = orig_path
        sys.meta_path[:] = orig_meta_path
        for name, previous in orig_modules.items():
            if previous is missing:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = previous

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
