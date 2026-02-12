"""Purpose: differential coverage for struct pack_into/unpack_from/iter_unpack."""

import struct

buf = bytearray(8)
struct.pack_into("ii", buf, 0, 1, 2)
print(struct.unpack_from("ii", buf, 0))
print(list(struct.iter_unpack("i", buf)))
