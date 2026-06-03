"""Purpose: CPython parity for builtin-method error messages/behavior that the
runtime hardcoded incorrectly or for the wrong version. Several of these messages
changed across CPython 3.12/3.13/3.14, so the runtime gates on the target version
(runtime_target_at_least); the differential harness derives Molt's target from
the CPython under test, so this file must match on each of 3.12/3.13/3.14.

Covers: divmod-by-zero (int/bigint/float/mixed), list.pop/insert non-int index,
list.index not-found (repr-based <3.14, static 3.14), dict.update non-iterable
element, set.remove missing key (KeyError(key)), bytes/bytearray.fromhex parsing,
float.fromhex non-str argument.
"""


def show(label, fn):
    try:
        r = fn()
        print(label, "OK", repr(r))
    except Exception as e:
        print(label, type(e).__name__, str(e))


# divmod by zero: "integer division or modulo by zero" / "float divmod()" on
# 3.12/3.13, unified to "division by zero" on 3.14.
show("divmod_int", lambda: divmod(7, 0))
show("divmod_big", lambda: divmod(10**40, 0))
show("divmod_float", lambda: divmod(7.0, 0.0))
show("divmod_mixed", lambda: divmod(7, 0.0))

# list pop/insert with a non-integer index -> "'<type>' object cannot be
# interpreted as an integer" (version-stable).
show("list_pop_str", lambda: [1, 2, 3].pop("x"))
show("list_pop_float", lambda: [1, 2, 3].pop(1.5))
show("list_insert_str", lambda: [1, 2, 3].insert("x", 9))

# list.index not found: "<repr(x)> is not in list" on 3.12/3.13, static
# "list.index(x): x not in list" on 3.14.
show("list_index_int", lambda: [1, 2, 3].index(99))
show("list_index_str", lambda: ["a", "b"].index("zz"))

# dict.update with a non-iterable sequence element: "cannot convert dictionary
# update sequence element #N to a sequence" on 3.12/3.13, "object is not
# iterable" on 3.14.
show("dict_update_badelem", lambda: {}.update([1]))

# set.remove missing key: KeyError(key) — str(e) == repr(key), not a message.
show("set_remove_missing", lambda: {1, 2, 3}.remove(5))
show("set_remove_str", lambda: {"a"}.remove("z"))

# bytes/bytearray.fromhex: whitespace allowed only between byte pairs (not inside
# a pair), \x0b accepted as a separator; odd trailing nibble -> position error
# (<3.14) or even-digits error (3.14).
show("fromhex_odd", lambda: bytes.fromhex("abc"))
show("fromhex_space_in_byte", lambda: bytes.fromhex("a b"))
show("fromhex_vtab_sep", lambda: bytes.fromhex("ab\x0bcd"))
show("fromhex_trailing_ws", lambda: bytes.fromhex("abc "))
show("fromhex_valid", lambda: bytes.fromhex("ab cd"))
show("bytearray_fromhex_odd", lambda: bytearray.fromhex("abc"))

# float.fromhex with a non-str argument: generic "bad argument type for built-in
# operation" on all versions.
show("float_fromhex_int", lambda: float.fromhex(5))
show("float_fromhex_bytes", lambda: float.fromhex(b"x"))
