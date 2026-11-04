"""Purpose: the runtime-owned PathFinder and FileFinder behave as CPython's
classes for subclasses and monkeypatching (HF-737)."""

import importlib.machinery as machinery
import os
import sys

directory = os.path.dirname(os.path.abspath(__file__))


class SubPathFinder(machinery.PathFinder):
    calls = []

    @classmethod
    def _get_spec(cls, fullname, path, target=None):
        cls.calls.append(fullname)
        if fullname == "importlib_default_finders_none":
            return None
        return super()._get_spec(fullname, path, target)


print(
    "none",
    SubPathFinder.find_spec("importlib_default_finders_none", [directory]) is None,
)
spec = SubPathFinder.find_spec("importlib_default_finders_pkg", [directory])
print("spec", spec.name, type(spec.loader).__name__, spec.origin.endswith("__init__.py"))
print("subclass calls", SubPathFinder.calls)
print(
    "missing",
    machinery.PathFinder.find_spec("importlib_default_finders_missing", [directory]),
)


class SubFileFinder(machinery.FileFinder):
    pass


hook = SubFileFinder.path_hook()
sub_finder = hook(directory)
print("subclass hook", type(sub_finder).__name__, sub_finder.path == directory)
print(
    "subclass find_spec",
    sub_finder.find_spec("importlib_default_finders_pkg").name,
)

original = vars(machinery.PathFinder)["find_spec"]
seen = []


def patched(cls, fullname, path=None, target=None):
    seen.append(fullname)
    return original.__get__(None, cls)(fullname, path, target)


machinery.PathFinder.find_spec = classmethod(patched)
try:
    try:
        import importlib_default_finders_patched  # noqa: F401
    except ModuleNotFoundError as exc:
        print("patched miss", exc.name)
finally:
    machinery.PathFinder.find_spec = original
print("patched seen", "importlib_default_finders_patched" in seen)
print("restored", vars(machinery.PathFinder)["find_spec"] is original)
print("meta_path has PathFinder", machinery.PathFinder in sys.meta_path)
