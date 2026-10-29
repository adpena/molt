"""Purpose: a namespace package found on the path has CPython 3.12's shape.

PathFinder.find_spec returns a loader-less spec with origin None whose
submodule_search_locations is a _NamespacePath, and module_from_spec binds a
NamespaceLoader, __file__ None and that same path object. The path recomputes
its portions when the parent path changes or PathFinder.invalidate_caches runs,
as importlib._bootstrap_external._NamespacePath does.
"""

import importlib.machinery
import importlib.util
import os
import sys
import tempfile

NAME = "molt_ns_path_protocol"
root = os.path.realpath(tempfile.mkdtemp(prefix="molt_ns_path_"))
first, second, third = (os.path.join(root, part) for part in ("a", "b", "c"))
os.makedirs(os.path.join(first, NAME))
os.makedirs(os.path.join(second, NAME))
os.makedirs(third)


def show(paths):
    return [os.path.relpath(path, root) for path in paths]


saved_path = sys.path[:]
saved_cache = dict(sys.path_importer_cache)
try:
    # The interpreter's own entries stay: invalidate_caches may import.
    sys.path[:] = [first, *saved_path]
    spec = importlib.machinery.PathFinder.find_spec(NAME)
    print("spec", spec.loader, spec.origin, spec.has_location, spec.cached, spec.parent)
    locations = spec.submodule_search_locations
    print("type", type(locations).__name__)
    print("repr", repr(locations).replace(root, "<root>"))
    print("len", len(locations))
    print("contains", os.path.join(first, NAME) in locations, "x" in locations)
    print("getitem", os.path.relpath(locations[0], root), show(locations[0:]))
    print("iter", show(locations))

    module = importlib.util.module_from_spec(spec)
    loader = module.__loader__
    print(
        "loader",
        type(loader).__name__,
        isinstance(loader, importlib.machinery.NamespaceLoader),
        loader is spec.loader,
        loader._path is locations,
        loader.is_package(NAME),
    )
    print(
        "module",
        module.__file__,
        module.__path__ is locations,
        module.__spec__ is spec,
        module.__package__,
    )

    sys.path.append(second)
    print("parent path change", show(module.__path__), len(module.__path__))

    # A portion that appears under an entry already on the path stays unseen
    # until the parent path changes or the caches are invalidated.
    os.makedirs(os.path.join(third, NAME))
    sys.path.insert(1, third)
    print("third portion", show(module.__path__))
    late = os.path.join(root, "late")
    os.makedirs(late)
    sys.path.append(late)
    print("late entry", show(module.__path__))
    os.makedirs(os.path.join(late, NAME))
    print("unchanged parent path", show(module.__path__))
    importlib.machinery.PathFinder.invalidate_caches()
    print("after invalidate_caches", show(module.__path__))

    module.__path__.append(os.path.join(root, "appended"))
    print("append", show(module.__path__))
    module.__path__[0] = os.path.join(root, "replaced")
    print("setitem", show(module.__path__))
finally:
    sys.path[:] = saved_path
    sys.path_importer_cache.clear()
    sys.path_importer_cache.update(saved_cache)
