"""Purpose: differential coverage for bytes bytearray ops."""

b = bytes.fromhex("00 ff")
print(b.hex())

ba = bytearray(b"abc")
ba[0] = ord("z")
print(bytes(ba).decode())

trans = bytes.maketrans(b"abc", b"xyz")
print(b"cab".translate(trans))
print(bytes.fromhex("61 62 63"))
