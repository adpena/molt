lst = [1, 2, 3]
lst.extend([4, 5])
lst.insert(1, 99)
lst.remove(2)
print(lst)

d = {"a": 1, "b": 2}
print(d.pop("a"))
print(d.pop("c", 9))
items = d.items()
pair_sum = 0
for pair in items:
    pair_sum = pair_sum + pair[1]
print(pair_sum)

t = (1, 2, 1)
print(t.count(1))
print(t.index(2))

total = 0
for x in [1, 2, 3]:
    total = total + x
print(total)
acc = 0
for x in (4, 5):
    acc = acc + x
print(acc)

d2 = {1: 10, 2: 20}
sumk = 0
for x in d2.keys():
    sumk = sumk + x
print(sumk)
sumv = 0
for x in d2.values():
    sumv = sumv + x
print(sumv)


def sum_items(mapping):
    total = 0
    for pair in mapping.items():
        total = total + pair[1]
    return total


print(sum_items(d2))


def dict_methods_dynamic(mapping):
    print(mapping.get("missing"))
    print(mapping.get("missing", 99))
    print(mapping.pop("a"))
    print(mapping.pop("missing", 123))


dict_methods_dynamic({"a": 1})

s = "a☃b"
chars: list[str] = []
for ch in s:
    chars.append(ch)
print(chars)

b = b"\x00\xff"
byte_vals: list[int] = []
for val in b:
    byte_vals.append(val)
print(byte_vals)

ba = bytearray(b"\x01\x02")
ba_vals: list[int] = []
for val in ba:
    ba_vals.append(val)
print(ba_vals)
