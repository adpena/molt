data = bytearray(b"abcd")
mv = memoryview(data)
print(mv[1])
mv[2] = 120
print(mv.tobytes())
print(mv[1:4].tobytes())

stride = mv[::2]
print(stride.tobytes())
stride[1] = 122
print(mv.tobytes())

ro = memoryview(b"hi")
print(ro[0])
print(ro.tobytes())
