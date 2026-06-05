"""Purpose: csv.get_dialect() must raise the CPython-exact unhashable-type
TypeError for ANY unhashable key, not just list/dict/set.

This pins the behavior reconciled between the two physical copies of the csv
runtime module (in-tree builtins/csv.rs and the molt-runtime-serial satellite).
Both copies must route a non-string dialect name through the general
hashability check (HashContext::DictKey), so:

  * an unhashable key -> TypeError with the 3.14 context-word message
    "cannot use 'X' as a dict key (unhashable type: 'X')" (bare
    "unhashable type: 'X'" on 3.12/3.13), and
  * a hashable-but-unknown key -> csv.Error("unknown dialect").

The `bytearray` case is the load-bearing one: the satellite previously
hardcoded a list/dict/set type check that missed bytearray (and every other
unhashable type), returning "unknown dialect" instead of TypeError on the
default native build while the in-tree copy was already correct. Version-stable
TypeError/csv.Error *kind*; the message text is version-gated and matched by the
differential harness against the same CPython that runs this file.
"""

import csv


def show(label, thunk):
    try:
        thunk()
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, "NO-EXCEPTION")


show("get_list", lambda: csv.get_dialect([1, 2]))
show("get_dict", lambda: csv.get_dialect({1: 2}))
show("get_set", lambda: csv.get_dialect({1, 2}))
show("get_bytearray", lambda: csv.get_dialect(bytearray(b"x")))
show("get_unknown_str", lambda: csv.get_dialect("molt_missing_dialect"))
