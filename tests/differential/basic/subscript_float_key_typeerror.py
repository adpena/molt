"""Purpose: differential coverage that sequence subscript enforces CPython's
integer protocol (`__index__`) for the index key.

CPython sequence subscript (tuple / list / bytes / bytearray / range / str)
accepts only objects implementing `__index__` (int, bool, int-subclass, or an
object defining `__index__`). A `float` — even an integral `2.0` — has no
`nb_index` and must raise `TypeError: <type> indices must be integers or
slices, not float`; it must NOT be silently truncated to an int index. This
locks read, assignment, and deletion across every sequence container, and
guards that the unaffected paths (bool / int-subclass / `__index__` keys, and
float value-membership which uses `==`, not `__index__`) keep working.
"""


def show(label, fn):
    try:
        print(f"{label}: {fn()!r}")
    except (TypeError, IndexError) as exc:
        print(f"{label}: {type(exc).__name__}: {exc}")


# --- float key on read: TypeError, never truncation to an int index ---
show("tuple_get_float", lambda: (10, 20, 30)[2.0])
show("list_get_float", lambda: [10, 20, 30][1.0])
show("bytes_get_float", lambda: b"abc"[1.0])
show("bytearray_get_float", lambda: bytearray(b"abc")[1.0])
show("range_get_float", lambda: range(5)[2.0])
show("str_get_float", lambda: "abc"[1.0])

# integral 0.0, negative float, and NaN float all raise too
show("list_get_zero_float", lambda: [10, 20, 30][0.0])
show("tuple_get_neg_float", lambda: (10, 20, 30)[-1.0])
show("list_get_nan_float", lambda: [10, 20, 30][float("nan")])

# other non-index keys raise the same TypeError family
show("list_get_str", lambda: [10, 20, 30]["1"])
show("list_get_none", lambda: [10, 20, 30][None])
show("range_get_str", lambda: range(5)["2"])


# --- float key on assignment: TypeError ---
def list_set_float():
    data = [10, 20, 30]
    data[1.0] = 99
    return data


def bytearray_set_float():
    data = bytearray(b"abc")
    data[1.0] = 65
    return bytes(data)


show("list_set_float", list_set_float)
show("bytearray_set_float", bytearray_set_float)


# --- float key on deletion: TypeError ---
def list_del_float():
    data = [10, 20, 30]
    del data[1.0]
    return data


def bytearray_del_float():
    data = bytearray(b"abc")
    del data[1.0]
    return bytes(data)


show("list_del_float", list_del_float)
show("bytearray_del_float", bytearray_del_float)


# --- regression guards: valid index keys still work ---
# bool is an int subclass -> a legitimate index (True == 1, False == 0)
show("list_get_true", lambda: [10, 20, 30][True])
show("list_get_false", lambda: [10, 20, 30][False])
show("tuple_get_true", lambda: (10, 20, 30)[True])
show("bytes_get_true", lambda: b"abc"[True])
show("range_get_true", lambda: range(10)[True])


class MyInt(int):
    pass


show("list_get_intsub", lambda: [10, 20, 30][MyInt(2)])
show("range_get_intsub", lambda: range(10)[MyInt(3)])


class Idx:
    def __index__(self):
        return 2


show("list_get_index_dunder", lambda: [10, 20, 30][Idx()])
show("tuple_get_index_dunder", lambda: (10, 20, 30)[Idx()])
show("range_get_index_dunder", lambda: range(10)[Idx()])
show("bytes_get_index_dunder", lambda: b"abc"[Idx()])
show("bytearray_get_index_dunder", lambda: bytearray(b"abc")[Idx()])


def list_set_index_dunder():
    data = [10, 20, 30]
    data[Idx()] = 99
    return data


def list_del_index_dunder():
    data = [10, 20, 30]
    del data[Idx()]
    return data


show("list_set_index_dunder", list_set_index_dunder)
show("list_del_index_dunder", list_del_index_dunder)

# negative valid index still works for read/write/delete
show("list_get_neg", lambda: [10, 20, 30][-1])
show("tuple_get_neg", lambda: (10, 20, 30)[-2])


# --- value-membership uses ==, NOT __index__: a float equal to an int matches.
# This path is independent of the index-coercion fix and must stay correct. ---
show("float_in_list", lambda: 2.0 in [1, 2, 3])
show("float_in_tuple", lambda: 2.0 in (1, 2, 3))
show("nonint_float_in_list", lambda: 2.5 in [1, 2, 3])


# --- huge index magnitude -> IndexError (overflow / out of range), NOT
# TypeError. The key IS an integer; it just does not fit. ---
show("list_get_bigint", lambda: [10, 20, 30][10**100])
show("range_get_bigint", lambda: range(5)[10**100])


class BigIdx:
    def __index__(self):
        return 10**100


show("list_get_big_index_dunder", lambda: [10, 20, 30][BigIdx()])
