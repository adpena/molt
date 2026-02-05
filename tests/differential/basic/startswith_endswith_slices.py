"""Purpose: verify startswith/endswith start/end slice behavior."""

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
