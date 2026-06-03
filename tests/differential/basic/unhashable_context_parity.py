"""Purpose: CPython-version parity for the unhashable-type `TypeError` message.

CPython 3.14 added an operation-specific context prefix to the bare
`unhashable type: 'X'` message:

    cannot use 'X' as a set element (unhashable type: 'X')   # set insert
    cannot use 'X' as a dict key (unhashable type: 'X')      # dict key

3.12 and 3.13 emit the bare form for every operation. Even on 3.14 the bare
form is still used for operations that merely *probe* a container by hashing
the candidate without inserting it into a fresh set:

    set.intersection / set.intersection_update / set.issubset

as well as the `hash()` builtin. This test pins all three behaviours so the
version-gated `HashContext` threading in `ensure_hashable` stays byte-identical
to CPython on 3.12 / 3.13 / 3.14.

(`collections.Counter([[]])` is also bare-on-every-version in CPython, but
molt's Counter is backed by a custom entry vector that compares keys with
`obj_eq` and never hashes them, so it silently accepts an unhashable list — a
SEPARATE pre-existing bug in the collections crate, upstream of
`ensure_hashable`. It is intentionally not exercised here so this lane stays a
clean regression for the message fix; the Counter hash-check gap is tracked
independently.)

The set literal, the `&` / `-` operators, the dict literal, the
`d[[]] = 1` statement and the membership tests are exercised at module scope
because a statement form cannot be wrapped in a lambda; the rest go through
the `show()` helper. Every offending value is a bare `[]` (list) so the type
name in the message is always `'list'`.

The dict.pop case uses a NON-empty dict (`{1: 2}.pop([])`): CPython
short-circuits `pop` on an *empty* dict to KeyError/default WITHOUT hashing the
key, so only a non-empty dict reaches the dict-key TypeError on every version.
"""


def show(label, fn):
    try:
        r = fn()
        print(label, "OK", repr(r))
    except Exception as e:
        print(label, type(e).__name__, str(e))


# ---------------------------------------------------------------------------
# set-element context (3.14 adds "set element"; bare on 3.12/3.13)
# ---------------------------------------------------------------------------
show("set_from_list", lambda: set([[]]))
show("set_add", lambda: set().add([]))
show("set_discard", lambda: set().discard([]))
show("set_remove", lambda: set().remove([]))
show("frozenset_from_list", lambda: frozenset([[]]))
show("set_update", lambda: set().update([[]]))
show("set_union", lambda: set().union([[]]))
show("set_difference", lambda: set().difference([[]]))
show("set_difference_update", lambda: set().difference_update([[]]))
show("set_symmetric_difference", lambda: set().symmetric_difference([[]]))
show("set_symmetric_difference_update", lambda: set().symmetric_difference_update([[]]))
show("set_issuperset", lambda: set().issuperset([[]]))
show("set_isdisjoint", lambda: set().isdisjoint([[]]))
show("set_comprehension", lambda: {x for x in [[]]})

# Membership / operator / literal forms at module scope.
try:
    _r = {[]}
    print("set_literal", "OK", repr(_r))
except Exception as e:
    print("set_literal", type(e).__name__, str(e))

try:
    _r = [] in set()
    print("set_in", "OK", repr(_r))
except Exception as e:
    print("set_in", type(e).__name__, str(e))

try:
    _r = {1} & set([[]])
    print("set_and", "OK", repr(_r))
except Exception as e:
    print("set_and", type(e).__name__, str(e))

try:
    _r = set() - set([[]])
    print("set_sub", "OK", repr(_r))
except Exception as e:
    print("set_sub", type(e).__name__, str(e))

# ---------------------------------------------------------------------------
# dict-key context (3.14 adds "dict key"; bare on 3.12/3.13)
# ---------------------------------------------------------------------------
show("dict_from_pairs", lambda: dict([([], 1)]))
show("dict_fromkeys", lambda: dict.fromkeys([[]]))
show("dict_setdefault", lambda: {}.setdefault([]))
show("dict_get", lambda: {}.get([]))
show("dict_pop_nonempty", lambda: {1: 2}.pop([]))
show("dict_comprehension", lambda: {k: 1 for k in [[]]})


def _csv_get_dialect():
    import csv

    return csv.get_dialect([])


show("csv_get_dialect", _csv_get_dialect)

# Subscript / membership / literal forms at module scope.
try:
    _d = {}
    _d[[]] = 1
    print("dict_setitem", "OK", repr(_d))
except Exception as e:
    print("dict_setitem", type(e).__name__, str(e))

try:
    _r = {}[[]]
    print("dict_getitem", "OK", repr(_r))
except Exception as e:
    print("dict_getitem", type(e).__name__, str(e))

try:
    _r = [] in {}
    print("dict_in", "OK", repr(_r))
except Exception as e:
    print("dict_in", type(e).__name__, str(e))

try:
    _r = {[]: 1}
    print("dict_literal", "OK", repr(_r))
except Exception as e:
    print("dict_literal", type(e).__name__, str(e))

# ---------------------------------------------------------------------------
# bare on EVERY version, even 3.14 (probe-only / hash builtin / Counter)
# ---------------------------------------------------------------------------
show("set_intersection", lambda: {1}.intersection([[]]))
show("set_intersection_update", lambda: {1}.intersection_update([[]]))
show("set_issubset", lambda: set().issubset([[]]))
show("hash_list", lambda: hash([]))
