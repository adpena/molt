class Idx:
    def __index__(self):
        return 1


class Step:
    def __index__(self):
        return 2


class BadIndex:
    def __index__(self):
        return "nope"


class OverflowIndex:
    def __index__(self):
        raise OverflowError("boom")


lst = [0, 1, 2, 3, 4]
lst[1:3] = [9, 8]
print(f"list_assign_basic:{lst}")

lst = [0, 1, 2, 3, 4]
lst[::2] = [7, 8, 9]
print(f"list_assign_step:{lst}")

lst = [0, 1, 2, 3, 4]
try:
    lst[::2] = [7]
except ValueError as exc:
    print(f"list_assign_step_err:{exc}")

lst = [0, 1, 2, 3, 4]
del lst[1:4]
print(f"list_del_slice:{lst}")

lst = [0, 1, 2, 3, 4]
del lst[::2]
print(f"list_del_step:{lst}")

lst = [0, 1, 2, 3]
lst[Idx() :] = [9, 9, 9]
print(f"list_assign_index:{lst}")

lst = [0, 1, 2, 3]
lst[:: Step()] = [6, 7]
print(f"list_assign_index_step:{lst}")

lst = [0, 1, 2, 3]
try:
    lst[BadIndex() :] = [1]
except TypeError as exc:
    print(f"list_assign_index_bad:{exc}")

lst = [0, 1, 2, 3]
try:
    lst[:: BadIndex()] = [1, 2]
except TypeError as exc:
    print(f"list_assign_step_bad:{exc}")

lst = [0, 1, 2, 3]
try:
    lst[OverflowIndex() :] = [1]
except OverflowError as exc:
    print(f"list_assign_index_overflow:{exc}")

ba = bytearray(b"abcde")
ba[1:4] = b"XYZ"
print(f"bytearray_assign_basic:{ba}")

ba = bytearray(b"abcde")
ba[::2] = [120, 121, 122]
print(f"bytearray_assign_step:{ba}")

ba = bytearray(b"abcde")
try:
    ba[::2] = [1]
except ValueError as exc:
    print(f"bytearray_assign_step_err:{exc}")

ba = bytearray(b"abcde")
del ba[1:4]
print(f"bytearray_del_slice:{ba}")

ba = bytearray(b"abcde")
del ba[::2]
print(f"bytearray_del_step:{ba}")

ba = bytearray(b"abcd")
ba[Idx() :] = b"ZZZ"
print(f"bytearray_assign_index:{ba}")

ba = bytearray(b"abcd")
try:
    ba[BadIndex() :] = b"x"
except TypeError as exc:
    print(f"bytearray_assign_index_bad:{exc}")

ba = bytearray(b"abcd")
try:
    ba[OverflowIndex() :] = b"x"
except OverflowError as exc:
    print(f"bytearray_assign_index_overflow:{exc}")

buf = bytearray(b"abcdef")
mv = memoryview(buf)
mv[1:3] = b"XY"
print(f"memoryview_assign_basic:{buf}")

buf = bytearray(b"abcdef")
mv = memoryview(buf)
try:
    mv[::2] = b"Z"
except ValueError as exc:
    print(f"memoryview_assign_step_err:{exc}")

buf = bytearray(b"abcdef")
mv = memoryview(buf)
try:
    del mv[1:3]
except TypeError as exc:
    print(f"memoryview_del_slice_err:{exc}")
