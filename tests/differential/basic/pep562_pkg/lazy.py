"""PEP 562 fixture: a module that defines BOTH ``__getattr__`` and ``__dir__``.

This is the canonical lazy-top-level-import idiom shape (numpy / scipy / rich /
sqlalchemy expose submodules and computed attributes through a module-level
``__getattr__``). All values returned here are computed *in Python at runtime*
from data already present in this statically-compiled module, so the feature is
fully static-graph-compatible: the import graph still resolves ``pep562_pkg.lazy``
at build time; only attribute *resolution* defers to ``__getattr__``.

STATIC-GRAPH BOUNDARY (the loud-refusal doctrine): a module ``__getattr__`` that
imported an arbitrary NOT-COMPILED module at runtime could not extend molt's
static link set — such an attribute would fail loudly (the referenced module is
absent from the AOT binary), exactly as any other runtime import of an
uncompiled module does. This fixture stays inside the supported subset by only
returning values derived from compiled-in data.
"""

# Real top-level binding: found by normal lookup, so __getattr__ is NOT called.
regular = 100

# Backing table for the lazily-resolved attributes.
_table = {"computed": 42, "heavy": "HEAVY"}

# Counts how many times __getattr__ fired, to prove found attributes bypass it.
_getattr_calls = 0


def __getattr__(name):
    # PEP 562: consulted only AFTER normal namespace lookup misses. Raising
    # AttributeError here is the contract for a genuinely-absent attribute.
    global _getattr_calls
    _getattr_calls += 1
    if name in _table:
        return _table[name]
    if name == "derived":
        # The recursion corner from PEP 562: a module __getattr__ that resolves
        # one lazy attribute IN TERMS OF another via the module itself (the
        # dispatch must be re-entrant — the self-recursion guard only suppresses
        # a __getattr__("__getattr__") lookup, never other names).
        import pep562_pkg.lazy as _self

        return "derived_from_" + _self.heavy
    raise AttributeError(f"module 'pep562_pkg.lazy' has no attribute {name!r}")


def __dir__():
    # PEP 562: overrides dir(module). CPython returns exactly this (sorted by
    # dir()). Deliberately unsorted + disjoint from the real namespace so a
    # regression that falls back to "list the module __dict__" is visible.
    return ["zeta", "alpha", "regular"]
