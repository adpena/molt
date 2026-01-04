nums = [1, 2, 3]

print((1 < 2, 2 <= 2, 3 > 2, 3 >= 4, 3 != 4, 3 == 3))

a = [1]
b = a
c = [1]
print((a is b, a is c, a is not c))

print((2 in nums, 4 in nums, 4 not in nums))
print(("a" in "cat", "z" in "cat"))
print((b"a" in b"cat", b"ab" in b"cat", b"" in b"cat"))
print((97 in b"cat", 97 in bytearray(b"cat")))
print((3 in range(1, 5), 6 in range(1, 5)))

print((1 < 2 < 3, 1 < 3 > 2, 1 < 2 > 3, 1 < 2 == 2))

print((not False, not 0, not 1))
