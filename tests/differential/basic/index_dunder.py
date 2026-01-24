"""Purpose: differential coverage for index dunder."""


class Idx:
    def __index__(self):
        return 1


class Big:
    def __index__(self):
        return 10**100


class NegBig:
    def __index__(self):
        return -(10**100)


class Bad:
    def __index__(self):
        return "x"


lst = [10, 20, 30]
print(f"list_idx:{lst[Idx()]}")
print(f"list_slice:{lst[Idx() : Idx() : Idx()]}")
print(f"list_big_start:{lst[Big() :]}")
print(f"list_big_step:{lst[:: Big()]}")
print(f"list_neg_step:{lst[:: NegBig()]}")

try:
    lst[Big()]
except IndexError as exc:
    print(f"list_big_err:{exc}")

try:
    lst[Bad()]
except TypeError as exc:
    print(f"list_bad_err:{exc}")

try:
    lst[Bad() :]
except TypeError as exc:
    print(f"list_slice_bad:{exc}")

s = "xyz"
try:
    s["a"]
except TypeError as exc:
    print(f"str_bad_err:{exc}")

b = b"xyz"
print(f"bytes_idx:{b[Idx()]}")

try:
    b[Big()]
except IndexError as exc:
    print(f"bytes_big_err:{exc}")

ba = bytearray(b"xyz")
print(f"bytearray_idx:{ba[Idx()]}")

mv = memoryview(b"xyz")
print(f"mv_idx:{mv[Idx()]}")

try:
    mv[Big()]
except IndexError as exc:
    print(f"mv_big_err:{exc}")

rng = range(5)
print(f"range_idx:{rng[Idx()]}")

try:
    rng[Big()]
except IndexError as exc:
    print(f"range_big_err:{exc}")
