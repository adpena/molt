# MOLT_ENV: PYTHONPATH=src:tests/differential/basic
"""Purpose: PEP 562 module-level ``__getattr__`` / ``__dir__`` parity.

PEP 562 (https://peps.python.org/pep-0562/, PSF) lets a module define
``__getattr__(name)`` — consulted on an attribute miss AFTER normal namespace
lookup, raising AttributeError if the name is genuinely absent — and
``__dir__()`` — which overrides ``dir(module)``. This is the lazy-top-level
-import idiom behind numpy / scipy / rich / sqlalchemy (doc 24 D16, Lane D
rank 1: the single highest-ROI ecosystem feature).

Corners exercised (CPython 3.12 / 3.13 / 3.14, byte-identical):
  1. a found top-level attribute does NOT call ``__getattr__``;
  2. an attribute miss routes to ``__getattr__`` and returns its value;
  3. ``__getattr__`` raising AttributeError preserves the message identity;
  4. ``from module import <missing>`` consults ``__getattr__`` first, then
     raises ImportError naming the attribute (the absolute origin path in
     CPython's message is environment-specific, so only the type + the named
     attribute are asserted — never the path);
  5. ``__dir__()`` overrides ``dir(module)`` (CPython returns exactly the
     ``__dir__`` result, sorted by ``dir()``);
  6. a module defining NEITHER hook is unchanged: a miss raises the stock
     module AttributeError and ``dir()`` lists the namespace.

STATIC-GRAPH BOUNDARY (documented in pep562_pkg/lazy.py): module ``__getattr__``
is static-graph-compatible because the import graph still resolves the module at
build time; only attribute *resolution* defers. A ``__getattr__`` body that
imports an arbitrary NOT-COMPILED module at runtime cannot extend the static
link set — such an attribute fails loudly (loud-refusal doctrine), exactly like
any other runtime import of an uncompiled module. This fixture stays inside the
supported subset.

The miss site wired in the runtime: module attribute access (MODULE_GET_ATTR ->
molt_module_get_attr) and from-import (MODULE_IMPORT_FROM -> molt_module_import_
from) both route the dict-miss through builtins/attr.rs::module_attr_lookup,
which dispatches the module-dict ``__getattr__`` (with a self-recursion guard);
dir() consults the module dict for ``__dir__`` in object/ops_builtins.rs::
molt_dir_builtin.
"""

from pep562_pkg import getattr_only, lazy, plain


def show(label, value):
    print(f"{label}: {value!r}")


# --- 1. found top-level attribute does NOT trigger __getattr__ ----------------
show("regular", lazy.regular)
show("getattr_calls_after_found", lazy._getattr_calls)

# --- 2. miss -> __getattr__ returns the computed value ------------------------
show("computed", lazy.computed)
show("heavy", lazy.heavy)
show("getattr_calls_after_two_misses", lazy._getattr_calls)

# --- 3. __getattr__ raising AttributeError: message identity ------------------
try:
    lazy.does_not_exist
except AttributeError as exc:
    show("attr_miss_type", type(exc).__name__)
    show("attr_miss_msg", str(exc))

# --- 3b. re-entrant __getattr__ (the PEP 562 recursion corner) ----------------
# `lazy.derived` resolves via __getattr__, whose body re-enters the same module's
# __getattr__ to read `lazy.heavy`. The dispatch must be re-entrant.
show("reentrant_derived", lazy.derived)

# --- 4. from-import of a missing name: __getattr__ first, then ImportError -----
# A present lazy attribute is importable via from-import (routes through
# __getattr__ before the import machinery would give up):
from pep562_pkg.lazy import computed as imported_computed

show("from_import_present", imported_computed)
try:
    from pep562_pkg.lazy import truly_absent  # noqa: F401
except ImportError as exc:
    # CPython's message embeds the module's absolute file origin, which differs
    # between the CPython reference run and molt's build tree. Assert only the
    # path-independent facts: the exception type and that it names the attribute.
    show("from_import_miss_type", type(exc).__name__)
    show("from_import_names_attr", "truly_absent" in str(exc))

# --- 5. __dir__ override: dir(module) == sorted(__dir__()) --------------------
# CPython's dir() materializes + STABLE-SORTS a __dir__ result (it never returns
# it verbatim): the fixture's __dir__ is deliberately unsorted, so this also
# proves dir() applies the sort to a module __dir__.
show("dir_override", dir(lazy))


# --- 5b. dir() post-processing parity (bug class shared with module __dir__) ---
# CPython's PyObject_Dir runs PySequence_List(result) then PyList_Sort: it SORTS
# but does NOT dedup, and accepts any iterable (tuple/generator), raising
# TypeError on a non-iterable. These corners are the same code path the module
# __dir__ override uses, so they live here next to the fix.
class _SortDup:
    def __dir__(self):
        return ["b", "a", "b", "c"]  # sorted, duplicates preserved


class _TupleDir:
    def __dir__(self):
        return ("z", "a")  # any iterable accepted, then sorted


class _BadDir:
    def __dir__(self):
        return 42  # non-iterable -> TypeError


show("dir_sort_keeps_dups", dir(_SortDup()))
show("dir_tuple_result", dir(_TupleDir()))
try:
    dir(_BadDir())
except TypeError as exc:
    show("dir_noniter_type", type(exc).__name__)
    show("dir_noniter_msg", str(exc))

# --- 6. a module without either hook is unchanged -----------------------------
show("plain_x", plain.x)
show("plain_y", plain.y)
plain_dir = dir(plain)
show("plain_dir_has_x_y", ("x" in plain_dir) and ("y" in plain_dir))
try:
    plain.absent
except AttributeError as exc:
    show("plain_miss_type", type(exc).__name__)
    show("plain_miss_msg", str(exc))

# --- 7. dir() on a module with __getattr__ but NO __dir__ ----------------------
# PEP 562: the module type's __dir__ slot reads the module __dict__ for a
# __dir__ entry DIRECTLY; it never routes that lookup through the module-level
# __getattr__. So dir() must (a) not consult __getattr__ for "__dir__" and
# (b) list the namespace as usual.
ga_dir = dir(getattr_only)
show("ga_dir_has_present", "present" in ga_dir)
show("ga_dir_did_not_call_getattr", getattr_only._dir_probe_calls == 0)
# And __getattr__ still works for a genuine miss on this module:
try:
    getattr_only.nope
except AttributeError as exc:
    show("ga_miss_type", type(exc).__name__)
    show("ga_getattr_calls_after_miss", getattr_only._dir_probe_calls)
