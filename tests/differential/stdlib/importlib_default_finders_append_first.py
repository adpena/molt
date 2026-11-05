"""Purpose: a finder and a hook appended before the first import keep the
default finders, which a cold import then consults before them (HF-737)."""

import sys

DEFAULTS = ("BuiltinImporter", "FrozenImporter", "PathFinder")
seen = []


class RecordingFinder:
    def find_spec(self, fullname, path=None, target=None):
        seen.append(fullname)
        return None


def recording_hook(path):
    raise ImportError("not handled")


def names():
    out = []
    for finder in sys.meta_path:
        name = finder.__name__ if isinstance(finder, type) else type(finder).__name__
        if name in DEFAULTS or name == "RecordingFinder":
            out.append(name)
    return out


sys.meta_path.append(RecordingFinder())
sys.path_hooks.append(recording_hook)
print("before", names(), sys.path_hooks[-1] is recording_hook)

import importlib_default_finders_pkg  # noqa: E402

print("sibling", importlib_default_finders_pkg.X)
try:
    import importlib_default_finders_missing  # noqa: F401
except ModuleNotFoundError as exc:
    print("missing", exc.name)
print(
    "seen",
    "importlib_default_finders_missing" in seen,
    "importlib_default_finders_pkg" in seen,
)
print("after", names())

import importlib.machinery  # noqa: E402

print(
    "PathFinder precedes RecordingFinder",
    sys.meta_path.index(importlib.machinery.PathFinder)
    < [type(f).__name__ for f in sys.meta_path].index("RecordingFinder"),
)
