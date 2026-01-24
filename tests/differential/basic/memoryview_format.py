"""Purpose: differential coverage for memoryview format."""

data = b"abcd"

mv = memoryview(data)
print(mv.format)
print(mv.shape)
print(mv.strides)
print(mv.ndim)
print(mv.itemsize)
print(mv.readonly)
print(mv.nbytes)

mv2 = mv[1:4:2]
print(mv2.shape)
print(mv2.strides)
print(mv2.nbytes)

mv3 = memoryview(bytearray(b"xyz"))
print(mv3.readonly)
print(mv3.format)
print(mv3.shape)
