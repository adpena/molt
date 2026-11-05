# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: CPython's default finders and hooks exist before the first import.

The runtime seeds them at bootstrap (HF-737); no import completes them later.
Site-installed finders, if any, are filtered out of the printed shapes. The
directory-backed hook checks live in importlib_default_finders_subclass.
"""

import sys

DEFAULTS = ("BuiltinImporter", "FrozenImporter", "PathFinder")


def default_shape(finders):
    return [
        (finder.__module__, finder.__qualname__)
        for finder in finders
        if isinstance(finder, type) and finder.__name__ in DEFAULTS
    ]


before = default_shape(sys.meta_path)
print("meta_path before", before)

import importlib_default_finders_pkg  # noqa: E402  (a path entry: the script directory)

print("sibling", importlib_default_finders_pkg.X)
print("meta_path kept", default_shape(sys.meta_path) == before)

import importlib  # noqa: E402
import importlib.machinery as machinery  # noqa: E402
import _frozen_importlib_external  # noqa: E402

print("PathFinder in meta_path", machinery.PathFinder in sys.meta_path)
print(
    "external identity",
    _frozen_importlib_external.PathFinder is machinery.PathFinder,
    _frozen_importlib_external.FileFinder is machinery.FileFinder,
)
for cls in (machinery.PathFinder, machinery.FileFinder):
    print("class", cls.__module__, cls.__qualname__)
members = vars(machinery.PathFinder)
print(
    "member types",
    type(members["find_spec"]).__name__,
    type(members["_get_spec"]).__name__,
    type(members["invalidate_caches"]).__name__,
    type(vars(machinery.FileFinder)["path_hook"]).__name__,
)
print(
    "frozen find_spec",
    machinery.FrozenImporter.find_spec("importlib_default_finders_absent") is None,
)

print("hook count", len(sys.path_hooks) >= 2)
try:
    sys.path_hooks[0]("importlib_default_finders_not_an_archive")
except ImportError as exc:
    # CPython's hook is zipimporter (ZipImportError); Molt's lazy archive hook
    # raises ImportError. Both reject with an ImportError.
    print("archive hook rejects a non-archive path", isinstance(exc, ImportError))

print("invalidate", importlib.invalidate_caches() is None)
print("meta_path kept after invalidate", default_shape(sys.meta_path) == before)
try:
    import importlib_default_finders_missing  # noqa: F401
except ModuleNotFoundError as exc:
    print("missing", exc.name)
