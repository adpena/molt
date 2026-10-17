"""Purpose: CPython parity for builtin-method error messages/behavior that the
runtime hardcoded incorrectly or for the wrong version. Several of these messages
changed across CPython 3.12/3.13/3.14, so the runtime gates on the target version
(runtime_target_at_least); the differential harness derives Molt's target from
the CPython under test, so this file must match the selected target version.

Covers: divmod-by-zero (int/bigint/float/mixed), list.pop/insert non-int index,
list.index not-found (repr-based <3.14, static 3.14), dict.update non-iterable
element, set.remove missing key (KeyError(key)), bytes/bytearray.fromhex parsing,
float.fromhex non-str argument.
"""

import io
import contextvars
import types


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
show("list_pop_none", lambda: [1, 2, 3].pop(None))
show("list_pop_default", lambda: [1, 2, 3].pop())
show("list_bound_pop_none", lambda: getattr([1, 2, 3], "pop")(None))
show("list_insert_str", lambda: [1, 2, 3].insert("x", 9))

# The same argument binder serves bytearray.pop: omission is -1, not None.
show("bytearray_pop_none", lambda: bytearray(b"abc").pop(None))
show("bytearray_pop_default", lambda: bytearray(b"abc").pop())
show("bytearray_bound_pop_none", lambda: getattr(bytearray(b"abc"), "pop")(None))
show("bytearray_pop_huge_empty", lambda: bytearray().pop(10**100))

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


# Index conversion runs before the operation reads the current length.
class AppendingIndex:
    def __init__(self, target):
        self.target = target

    def __index__(self):
        self.target.append(44)
        return -1


empty = []
show("list_pop_index_grows_empty", lambda: empty.pop(AppendingIndex(empty)))
print("list_pop_remaining", empty)

empty_bytes = bytearray()
show(
    "bytearray_pop_index_grows_empty",
    lambda: empty_bytes.pop(AppendingIndex(empty_bytes)),
)
print("bytearray_pop_remaining", empty_bytes)


# METH_NOARGS: descriptor receiver validation precedes keyword/count errors.
# Bound methods use the receiver type's qualified name; explicitly called
# descriptors use their declaring owner, even for a subclass receiver.
def noargs_errors(label, bound, descriptor, receiver):
    show(label + "_bound_pos", lambda: bound(1))
    show(label + "_bound_two", lambda: bound(1, 2))
    show(label + "_bound_kw", lambda: bound(extra=1))
    show(label + "_bound_both", lambda: bound(1, extra=2))
    show(label + "_explicit_pos", lambda: descriptor(receiver, 1))
    show(label + "_explicit_kw", lambda: descriptor(receiver, extra=1))
    show(label + "_explicit_both", lambda: descriptor(receiver, 1, extra=2))
    show(label + "_missing", lambda: descriptor())
    show(label + "_missing_kw", lambda: descriptor(extra=1))
    show(label + "_wrong_receiver", lambda: descriptor(None, 1, extra=2))


view = memoryview(b"abc")
noargs_errors("view_enter", view.__enter__, memoryview.__enter__, view)
noargs_errors("view_release", view.release, memoryview.release, view)
show("view_enter_success", lambda: view.__enter__() is view)
show("view_explicit_enter_success", lambda: memoryview.__enter__(view) is view)
show("view_release_success", lambda: view.release())
show("view_released_enter", lambda: view.__enter__())
# Argument validation still wins over the released-view operation error.
show("view_released_enter_extra", lambda: view.__enter__(1))
show("view_released_enter_keyword", lambda: view.__enter__(extra=1))

noargs_errors("bytes_lower", b"ABC".lower, bytes.lower, b"ABC")
noargs_errors("int_bits", (7).bit_length, int.bit_length, 7)
noargs_errors("list_reverse", [1, 2].reverse, list.reverse, [1, 2])
noargs_errors("dict_clear", {1: 2}.clear, dict.clear, {1: 2})
noargs_errors("set_copy", {1}.copy, set.copy, {1})
noargs_errors("set_clear", {1}.clear, set.clear, {1})
noargs_errors("frozen_copy", frozenset({1}).copy, frozenset.copy, frozenset({1}))


class NoArgsInt(int):
    pass


class NoArgsList(list):
    pass


class NoArgsSet(set):
    pass


integer_sub = NoArgsInt(7)
list_sub = NoArgsList([1, 2])
set_sub = NoArgsSet({1})
noargs_errors("int_sub_bits", integer_sub.bit_length, int.bit_length, integer_sub)
noargs_errors("list_sub_reverse", list_sub.reverse, list.reverse, list_sub)
noargs_errors("set_sub_copy", set_sub.copy, set.copy, set_sub)
# Exercise source CALL and CALL_FUNCTION_EX ingress as well as stored methods.
show("int_sub_direct_pos", lambda: integer_sub.bit_length(1))
show("int_sub_explicit_direct_pos", lambda: int.bit_length(integer_sub, 1))
show("int_sub_direct_keyword", lambda: integer_sub.bit_length(extra=1))
show("int_sub_direct_both", lambda: integer_sub.bit_length(1, extra=2))
show("int_sub_expanded_pos", lambda: integer_sub.bit_length(*(1,)))
show("int_sub_expanded_kw", lambda: integer_sub.bit_length(**{"extra": 1}))
show("int_sub_expanded_both", lambda: integer_sub.bit_length(*(1,), **{"extra": 2}))
show("int_sub_explicit_expanded_pos", lambda: int.bit_length(*(integer_sub, 1)))
show(
    "int_sub_explicit_expanded_kw",
    lambda: int.bit_length(*(integer_sub,), **{"extra": 1}),
)
show(
    "int_sub_explicit_expanded_both",
    lambda: int.bit_length(*(integer_sub, 1), **{"extra": 2}),
)
show("bytes_lower_success", lambda: b"ABC".lower())
show("int_sub_bits_success", lambda: integer_sub.bit_length())
show("int_sub_explicit_success", lambda: int.bit_length(integer_sub))
show("list_sub_reverse_success", lambda: list_sub.reverse())
print("list_sub_reversed", list_sub)
show("set_sub_copy_success", lambda: sorted(set_sub.copy()))
show("frozen_copy_success", lambda: sorted(frozenset({1}).copy()))
cleared = {1: 2}
show("dict_clear_success", lambda: cleared.clear())
print("dict_after_clear", cleared)

# Native IO declarations use the same contract and retain their owner label.
stream = io.BytesIO(b"abc")
noargs_errors("io_getvalue", stream.getvalue, io.BytesIO.getvalue, stream)
noargs_errors("io_readable", stream.readable, io.BytesIO.readable, stream)
show("io_getvalue_success", lambda: stream.getvalue())
show("io_readable_success", lambda: stream.readable())

# Receiver admission uses native tp_name on both sides; method arity still
# uses __qualname__. Python heap subtypes retain a short tp_name even when
# their module and qualified name differ, then follow an actual name change.
show("cname_actual_io", lambda: int.bit_length(stream))
show("cname_actual_namespace", lambda: list.reverse(types.SimpleNamespace()))
show("cname_owner_context", lambda: contextvars.Context.copy(1))


class CNameStream(io.BytesIO):
    pass


cname_stream = CNameStream(b"name")
CNameStream.__module__ = "changed_module"
CNameStream.__qualname__ = "Outer.Stream"
show("cname_heap_received", lambda: int.bit_length(cname_stream))
cname_bound = cname_stream.getvalue
show("cname_heap_bound_arity", lambda: cname_bound(1))
CNameStream.__name__ = "RenamedStream"
show("cname_heap_renamed", lambda: int.bit_length(cname_stream))
show("cname_heap_bound_after_name", lambda: cname_bound(1))
show("cname_heap_success", lambda: cname_bound())
cname_stream.close()
stream.close()


class NativeOuter:
    class Inner(int):
        pass


nested = NativeOuter.Inner(7)
nested_bound = nested.bit_length
show("nested_stored", lambda: nested_bound(1))
show("nested_direct", lambda: nested.bit_length(1))
NativeOuter.Inner.__qualname__ = "ChangedQualname"
show("nested_renamed_stored", lambda: nested_bound(1))
show("nested_renamed_direct", lambda: nested.bit_length(1))
NativeOuter.Inner.__name__ = "ChangedName"
show("nested_name_only_stored", lambda: nested_bound(1))


def suspended_native_call():
    integer_sub.bit_length((yield "native_argument"))


suspended = suspended_native_call()
print("native_suspended_start", next(suspended))
show("native_suspended_finish", lambda: suspended.send(1))
