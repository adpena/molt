buf = bytearray(b"abcdef")
rev = memoryview(buf)[::-1]
skip = memoryview(buf)[4:1:-2]

print(rev.tobytes())
print(skip.tobytes())
print(rev.tolist())
print(skip.tolist())
print(rev[0], rev[2], skip[1])
print(97 in rev, 102 in rev, b"cd" in rev)
print(memoryview(b"")[::-1].tobytes())
print(memoryview(bytearray(b""))[::-1].tolist())

rev[1:4:2] = b"XY"
print(buf)
