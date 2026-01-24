"""Purpose: differential coverage for augassign inplace."""


class Count:
    def __index__(self):
        return 2


class Box:
    def __init__(self, items, count):
        self.items = items
        self.count = count


lst = [1, 2]
alias = lst
lst += [3]
print(f"list_alias:{lst is alias}")
print(f"list_val:{lst}")
lst += (4, 5)
print(f"list_tuple:{lst}")
lst += {"a": 6, "b": 7}
print(f"list_dict:{lst}")
lst += range(8, 10)
print(f"list_gen:{lst}")
lst *= Count()
print(f"list_mul:{lst}")
print(f"list_mul_alias:{lst is alias}")
lst *= 0
print(f"list_zero:{lst}")

lst2 = [1, 2, 3]
lst2[1] += 10
print(f"list_sub:{lst2}")

box = Box([1], 1)
items_alias = box.items
box.items += [2]
print(f"attr_list:{box.items}")
print(f"attr_list_alias:{box.items is items_alias}")
box.count += 4
print(f"attr_int:{box.count}")

ba = bytearray(b"hi")
ba_alias = ba
ba += b"!"
print(f"ba_alias:{ba is ba_alias}")
print(f"ba_val:{bytes(ba)}")
ba += memoryview(b"yo")
print(f"ba_mem:{bytes(ba)}")
ba *= Count()
print(f"ba_mul:{bytes(ba)}")
ba *= 0
print(f"ba_zero:{bytes(ba)}")

try:
    ba = bytearray(b"a")
    ba += 1
except TypeError as exc:
    print(f"ba_add_err:{exc}")

try:
    ba = bytearray(b"a")
    ba *= 1.5
except TypeError as exc:
    print(f"ba_mul_err:{exc}")

ba2 = bytearray(b"abc")
ba2[1] += 1
print(f"ba_sub:{bytes(ba2)}")

s = {1, 2}
s_alias = s
s |= {2, 3}
print(f"set_or:{sorted(s)}")
print(f"set_or_alias:{s is s_alias}")
s &= {2, 3}
print(f"set_and:{sorted(s)}")
s -= {2}
print(f"set_sub:{sorted(s)}")
s ^= {5}
print(f"set_xor:{sorted(s)}")

view_src = {1: 10, 2: 20}
s = {0}
s |= view_src.keys()
print(f"set_or_keys:{sorted(s)}")
s = {0, 1, 2}
s &= view_src.keys()
print(f"set_and_keys:{sorted(s)}")
s = {0, 1, 2}
s -= view_src.keys()
print(f"set_sub_keys:{sorted(s)}")
s = {0, 1, 2}
s ^= view_src.keys()
print(f"set_xor_keys:{sorted(s)}")

s = {(1, 10)}
s |= view_src.items()
print(f"set_or_items:{sorted(s)}")
s = {(1, 10), (3, 30)}
s &= view_src.items()
print(f"set_and_items:{sorted(s)}")
s = {(1, 10), (3, 30)}
s -= view_src.items()
print(f"set_sub_items:{sorted(s)}")
s = {(1, 10), (3, 30)}
s ^= view_src.items()
print(f"set_xor_items:{sorted(s)}")

try:
    s = {1}
    s |= view_src.values()
except TypeError as exc:
    print(f"set_or_values_err:{exc}")

try:
    s = {1}
    s &= 1
except TypeError as exc:
    print(f"set_and_err:{exc}")

fs = frozenset({1, 2})
fs_alias = fs
fs |= {2, 3}
print(f"frozenset_or:{sorted(fs)}")
print(f"frozenset_or_alias:{fs is fs_alias}")
