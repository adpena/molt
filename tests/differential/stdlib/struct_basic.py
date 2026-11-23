"""Purpose: differential coverage for struct pack/unpack basics."""

import struct

blob = struct.pack("id", 7, 2.5)
print(struct.calcsize("id"))
print(struct.unpack("id", blob))

s = struct.Struct("i")
print(s.pack(3))
print(s.unpack(s.pack(4)))

# A zero repeat in a native format aligns subsequent fields without consuming
# a value. A zero-width string still consumes one value and unpacks to b"".
layout = struct.Struct("@b0qb2h0s")
record = layout.pack(6, 7, 9, -11, b"ignored")
print(layout.size, record, layout.unpack(record))
print(list(layout.iter_unpack(record + record)))
print(struct.calcsize("<100000000i"))


class Index:
    def __index__(self):
        return -(1 << 63)


limits = struct.Struct("<qQ")
record = limits.pack(Index(), (1 << 64) - 1)
print(limits.unpack(record))
for signed, unsigned in ((-(1 << 63) - 1, 0), (0, 1 << 128)):
    try:
        limits.pack(signed, unsigned)
    except struct.error as error:
        print(str(error))
