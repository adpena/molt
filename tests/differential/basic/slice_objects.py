lst = [0, 1, 2, 3, 4, 5]
print(lst[1:5:2])
print(lst[::-1])

nums = (1, 2, 3, 4, 5)
print(nums[0:5:2])
print(nums[4:0:-2])

b = b"abcdef"
print(b[1:5:2])
print(b[::-1])

ba = bytearray(b"abcdef")
print(ba[1:5:2])
print(ba[::-1])

s = "abcdef"
print(s[1:5:2])
print(s[::-1])

print(slice(1, 5, 2))
print(slice(None, None, -1))
