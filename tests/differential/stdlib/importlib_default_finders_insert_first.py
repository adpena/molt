"""Purpose: a finder inserted first, before any import, serves a module and
the default finders follow it (HF-737)."""

import sys

DEFAULTS = ("BuiltinImporter", "FrozenImporter", "PathFinder")


class VirtualLoader:
    def create_module(self, spec):
        return None

    def exec_module(self, module):
        module.VALUE = "virtual"


class VirtualFinder:
    spec_class = None

    def find_spec(self, fullname, path=None, target=None):
        if fullname != "importlib_default_finders_virtual":
            return None
        return self.spec_class(fullname, VirtualLoader())


def names():
    out = []
    for finder in sys.meta_path:
        name = finder.__name__ if isinstance(finder, type) else type(finder).__name__
        if name in DEFAULTS or name == "VirtualFinder":
            out.append(name)
    return out


finder = VirtualFinder()
sys.meta_path.insert(0, finder)
print("before", names())

import importlib_default_finders_pkg  # noqa: E402

VirtualFinder.spec_class = type(importlib_default_finders_pkg.__spec__)
import importlib_default_finders_virtual  # noqa: E402

print("virtual", importlib_default_finders_virtual.VALUE)
print("virtual loader", type(importlib_default_finders_virtual.__loader__).__name__)
print("after", names())
