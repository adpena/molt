def section(name):
    print(f"--- {name} ---")


section("Bytearray Slice Assignment")
b = bytearray(b"0123456789")
print(b)
b[2:5] = b"abc"  # Replace 3 chars with 3
print(b)
b[2:5] = b"XYZ"  # Replace 3 with 3
print(b)
b[2:5] = b"M"  # Replace 3 with 1 (shrink)
print(b)
b[2:3] = b"LONG"  # Replace 1 with 4 (grow)
print(b)

section("List Self-Assignment")
lst = [0, 1, 2, 3, 4]
lst[:] = lst  # Should be safe
print(lst)
lst[1:4] = lst[1:4]
print(lst)
# Tricky: lst[1:4] = lst (assigning whole list to slice)
lst[1:4] = lst
print(len(lst))  # 2 (0) + 5 (inserted) + 1 (4) = 8?

section("Set Self-Update")
s = {1, 2, 3}
s |= s
print(s)
s &= s
print(s)
s -= s
print(s)
s ^= {1, 2}  # {1, 2}
print(s)
s ^= s
print(s)
