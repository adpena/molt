"""Purpose: differential coverage for struct pack/unpack basics."""

import struct

blob = struct.pack("id", 7, 2.5)
print(struct.calcsize("id"))
print(struct.unpack("id", blob))

s = struct.Struct("i")
print(s.pack(3))
print(s.unpack(s.pack(4)))
