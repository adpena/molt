"""Surface checks for core builtin type methods lowered into Rust."""

import sys


def show_exc(label, fn, *args):
    try:
        fn(*args)
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__)


# tuple

t = (1, 2, 3, 2)
print("tuple", t.count(2), t.index(2), t.index(2, 2), t.index(2, 0, 3))
show_exc("tuple_index_missing", t.index, 9)


# str

str_map = str.maketrans("ab", "cd")
inst_str_map = "".maketrans("ab", "cd")
print(
    "str_maketrans",
    str_map[ord("a")],
    str_map[ord("b")],
    inst_str_map[ord("a")],
    inst_str_map[ord("b")],
)
print("str_maketrans_identity", str.maketrans is "".maketrans)


# int

print(
    "int",
    (5).bit_count(),
    (-5).bit_count(),
    (7).as_integer_ratio(),
    (7).conjugate(),
    (7).is_integer(),
)


# float / complex

print("float", (1.5).conjugate(), (2.0).is_integer(), (2.5).is_integer())
print("float_ratio", (0.5).as_integer_ratio())
hex_text = (0.1).hex()
print("float_hex", hex_text, float.fromhex(hex_text), float.fromhex("0x1.8p+1"))
if sys.version_info >= (3, 14):
    print("float_from_number", float.from_number(3), float.from_number(True))
    show_exc("float_from_number_str", float.from_number, "3")

if sys.version_info >= (3, 14):
    print("complex_from_number", complex.from_number(3.5), complex.from_number(1 + 2j))
    show_exc("complex_from_number_str", complex.from_number, "3")


# bytes

print("bytes_index", b"abca".index(b"b"), b"abca".rindex(b"a", 1))
show_exc("bytes_index_missing", b"abca".index, b"z")
print(
    "bytes_pred",
    b"abc".isalpha(),
    b"ab1".isalnum(),
    b"123".isdigit(),
    b"\t".isspace(),
    b"abc".islower(),
    b"ABC".isupper(),
    b"Hello World".istitle(),
    b"abc".isascii(),
    bytes([0xFF]).isascii(),
)
print(
    "bytes_case",
    b"abc".capitalize(),
    b"AbC".swapcase(),
    b"hello world".title(),
)
print(
    "bytes_pad",
    b"a".center(4, b"-"),
    b"a".ljust(4, b"-"),
    b"a".rjust(4, b"-"),
    b"+12".zfill(5),
)
print(
    "bytes_misc",
    b"a\tb".expandtabs(),
    b"foobar".removeprefix(b"foo"),
    b"foobar".removesuffix(b"bar"),
)


# bytearray

ba = bytearray(b"ab")
ba.insert(1, ord("Z"))
print("bytearray_insert", ba)
print("bytearray_pop", ba.pop(), ba)
ba.append(ord("c"))
ba.remove(ord("a"))
print("bytearray_remove", ba)
if sys.version_info >= (3, 14):
    ba.resize(5)
    print("bytearray_resize", ba)
ba.reverse()
print("bytearray_reverse", ba)
print(
    "bytearray_surface",
    bytearray(b"abc").capitalize(),
    bytearray(b"AbC").swapcase(),
    bytearray(b"hello world").title(),
    bytearray(b"a").center(4, b"-"),
    bytearray(b"a").ljust(4, b"-"),
    bytearray(b"a").rjust(4, b"-"),
    bytearray(b"+12").zfill(5),
    bytearray(b"a\tb").expandtabs(),
    bytearray(b"foobar").removeprefix(b"foo"),
    bytearray(b"foobar").removesuffix(b"bar"),
)
print(
    "bytearray_pred",
    bytearray(b"abc").isalpha(),
    bytearray(b"ab1").isalnum(),
    bytearray(b"123").isdigit(),
    bytearray(b"\t").isspace(),
    bytearray(b"abc").islower(),
    bytearray(b"ABC").isupper(),
    bytearray(b"Hello World").istitle(),
    bytearray(b"abc").isascii(),
    bytearray([0xFF]).isascii(),
)
print(
    "bytearray_more",
    bytearray(b",").join([b"a", bytearray(b"b")]),
    bytearray(b"abca").index(b"b"),
    bytearray(b"abca").rindex(b"a", 1),
    bytearray(b"xyz").copy(),
    bytearray(b"ABC").lower(),
    bytearray(b"abc").upper(),
)
show_exc("bytearray_index_missing", bytearray(b"abca").index, b"z")


# memoryview

mv = memoryview(b"abca")
if sys.version_info >= (3, 14):
    print("memoryview", mv.count(97), mv.index(98), mv.hex(), mv.hex(":", 2))
else:
    print("memoryview", mv.hex(), mv.hex(":", 2))
print("memoryview_has_from_flags", hasattr(memoryview, "_from_flags"))
if hasattr(memoryview, "_from_flags"):
    print("memoryview_from_flags_identity", memoryview._from_flags is mv._from_flags)
    print(
        "memoryview_from_flags",
        memoryview._from_flags(b"ab", 0).readonly,
        memoryview._from_flags(bytearray(b"ab"), 1).readonly,
        memoryview._from_flags(memoryview(b"ab"), 1).readonly,
    )
    show_exc("memoryview_from_flags_writable", memoryview._from_flags, b"ab", 1)
ro = memoryview(bytearray(b"ab")).toreadonly()
print("memoryview_readonly", ro.readonly)
mv2 = memoryview(bytearray(b"ab"))
print("memoryview_release", mv2.release())


# range

r = range(1, 10, 2)
print("range", r.count(5), r.count(6), r.index(5))
show_exc("range_index_missing", r.index, 6)


class EqFive:
    def __eq__(self, other):
        return other == 5


print("range_custom", r.count(EqFive()), r.index(EqFive()))


# property / exception

prop = property(lambda self: 1)
print(
    "property",
    callable(property.getter),
    callable(property.setter),
    callable(property.deleter),
)
print("property_getter_type", type(property.getter(prop, lambda self: 2)).__name__)

exc = Exception("x")
exc.add_note("n1")
print("exception_note", exc.__notes__)
print("exception_with_traceback", exc.with_traceback(None) is exc)
show_exc("exception_with_traceback_type", exc.with_traceback, 1)
print("exception_subclass_dir", "add_note" in dir(ValueError), "with_traceback" in dir(ValueError))
print(
    "exception_group_dir",
    "add_note" in dir(ExceptionGroup),
    "with_traceback" in dir(ExceptionGroup),
    "derive" in dir(ExceptionGroup),
    "split" in dir(ExceptionGroup),
    "subgroup" in dir(ExceptionGroup),
)
