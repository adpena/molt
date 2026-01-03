import molt_msgpack
import molt_cbor

msgpack_bytes = b"\xc4\x02\x00\x01"
val = molt_msgpack.parse(msgpack_bytes)
print(val)
print(val.find(b"\x01"))
print(val.replace(b"\x00", b"\x02"))
ba = bytearray(val)
print(ba.find(b"\x00"))
print(len(ba.split(b"\x00")))

cbor_bytes = b"\x42hi"
val2 = molt_cbor.parse(cbor_bytes)
print(val2)
print(val2.find(b"h"))
