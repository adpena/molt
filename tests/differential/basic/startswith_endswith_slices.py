"""Purpose: verify startswith/endswith start/end slice behavior."""

# Exercise every specialized producer before callbacks invalidate module facts.
family_s = "abcabc"
family_b = b"abcabc"
family_ba = bytearray(b"abcabc")

family_s.find("bc")
family_s.find("bc", -5, 9)
family_s.startswith("bc")
family_s.startswith("bc", -5, 9)
family_s.endswith("bc")
family_s.endswith("bc", -5, 9)
family_s.count("bc")
family_s.count("bc", -5, 9)
family_b.find(b"bc")
family_b.find(b"bc", -5, 9)
family_b.startswith(b"bc")
family_b.startswith(b"bc", -5, 9)
family_b.endswith(b"bc")
family_b.endswith(b"bc", -5, 9)
family_b.count(b"bc")
family_b.count(b"bc", -5, 9)
family_ba.find(b"bc")
family_ba.find(b"bc", -5, 9)
family_ba.startswith(b"bc")
family_ba.startswith(b"bc", -5, 9)
family_ba.endswith(b"bc")
family_ba.endswith(b"bc", -5, 9)
family_ba.count(b"bc")
family_ba.count(b"bc", -5, 9)

family_results = (
    family_s.find("bc"),
    family_s.find("bc", -5, 9),
    family_s.startswith("bc"),
    family_s.startswith("bc", -5, 9),
    family_s.endswith("bc"),
    family_s.endswith("bc", -5, 9),
    family_s.count("bc"),
    family_s.count("bc", -5, 9),
    family_b.find(b"bc"),
    family_b.find(b"bc", -5, 9),
    family_b.startswith(b"bc"),
    family_b.startswith(b"bc", -5, 9),
    family_b.endswith(b"bc"),
    family_b.endswith(b"bc", -5, 9),
    family_b.count(b"bc"),
    family_b.count(b"bc", -5, 9),
    family_ba.find(b"bc"),
    family_ba.find(b"bc", -5, 9),
    family_ba.startswith(b"bc"),
    family_ba.startswith(b"bc", -5, 9),
    family_ba.endswith(b"bc"),
    family_ba.endswith(b"bc", -5, 9),
    family_ba.count(b"bc"),
    family_ba.count(b"bc", -5, 9),
)
print("shared-text-family", family_results)

s = "hello world"
print(s.startswith("hello", 0))
print(s.startswith("world", 6))
print(s.startswith("world", 7))
print(s.startswith(("he", "ha"), 0, 2))
print(s.startswith("world", -5))
print(s.endswith("world"))
print(s.endswith("world", 0))
print(s.endswith("hello", 0, 5))
print(s.endswith("hello", 0, 4))

b = b"abcabc"
print(b.startswith(b"abc", 0))
print(b.startswith(b"abc", 3))
print(b.startswith((b"ab", b"zz"), 3, 5))
print(b.endswith(b"abc", 0, 3))
print(b.endswith(b"abc", 0, 2))

ba = bytearray(b"xyzxyz")
print(ba.startswith(b"xyz", 0))
print(ba.startswith(b"xyz", 3))
print(ba.endswith(b"xyz", 0, 3))
print(ba.endswith((b"yz", b"aa"), 1, 3))


# Exercise method calls with parameter receivers, discarded results and wide bounds.
# CPython is the independent value/error oracle for each emitted program.
def string_calls(s: str, needle: str, wide: int):
    print(s.find(needle), s.startswith(needle), s.endswith(needle), s.count(needle))
    print(
        s.find(needle, -wide, wide),
        s.startswith(needle, -wide, wide),
        s.endswith(needle, -wide, wide),
        s.count(needle, -wide, wide),
    )
    s.find(needle)
    s.startswith(needle)
    s.endswith(needle)
    s.count(needle)
    s.find(needle, -wide, wide)
    s.startswith(needle, -wide, wide)
    s.endswith(needle, -wide, wide)
    s.count(needle, -wide, wide)


def bytes_calls(s: bytes, needle: bytes, wide: int):
    print(s.find(needle), s.startswith(needle), s.endswith(needle), s.count(needle))
    print(
        s.find(needle, -wide, wide),
        s.startswith(needle, -wide, wide),
        s.endswith(needle, -wide, wide),
        s.count(needle, -wide, wide),
    )
    s.find(needle)
    s.startswith(needle)
    s.endswith(needle)
    s.count(needle)
    s.find(needle, -wide, wide)
    s.startswith(needle, -wide, wide)
    s.endswith(needle, -wide, wide)
    s.count(needle, -wide, wide)


def bytearray_calls(s: bytearray, needle: bytes, wide: int):
    print(s.find(needle), s.startswith(needle), s.endswith(needle), s.count(needle))
    print(
        s.find(needle, -wide, wide),
        s.startswith(needle, -wide, wide),
        s.endswith(needle, -wide, wide),
        s.count(needle, -wide, wide),
    )
    s.find(needle)
    s.startswith(needle)
    s.endswith(needle)
    s.count(needle)
    s.find(needle, -wide, wide)
    s.startswith(needle, -wide, wide)
    s.endswith(needle, -wide, wide)
    s.count(needle, -wide, wide)


wide = (1 << 31) * (1 << 31)
string_calls("é😀é", "é", wide)
bytes_calls(b"ababa", b"a", wide)
bytearray_calls(bytearray(b"ababa"), b"a", wide)
string_calls("é😀é", "", wide)
bytes_calls(b"ababa", b"", wide)
bytearray_calls(bytearray(b"ababa"), b"", wide)


# Resultless calls must retain the same exception and callback boundary.
class TextBound:
    def __index__(self):
        print("bound-index")
        raise RuntimeError("text bound")


for value, needle in [("abc", "a"), (b"abc", b"a"), (bytearray(b"abc"), b"a")]:
    try:
        value.find(needle, TextBound(), None)
    except RuntimeError as error:
        print(type(error).__name__, str(error))
