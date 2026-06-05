"""Purpose: exercise the binascii SSE2/NEON hex-decode fast path (>=32-byte
even-length input drives the 32-byte SIMD decode loop) so the in-tree (micro
tier) and satellite (default tier) copies stay byte-identical after the Move
R.1b dead-SIMD-store removal. Both copies dropped a shadowed `result` SSE2
binding that was recomputed before use; this verifies the surviving
hi-nibble/lo-nibble combine is unchanged across the full byte range.
"""

import binascii

# a2b_hex over a long even-length hex string hits the 32-byte SSE2 decode loop.
hexstr = "0123456789abcdefABCDEF" * 8
data = binascii.a2b_hex(hexstr)
print("a2b_hex", binascii.b2a_hex(data).decode())
print("a2b_hex_len", len(data))

# Round-trip every byte value through hexlify/unhexlify (full 0x00..0xFF range,
# 256 bytes -> 512 hex chars -> 16 SIMD blocks).
allbytes = bytes(range(256))
hexed = binascii.hexlify(allbytes)
print("hexlify", hexed.decode())
print("unhexlify_ok", binascii.unhexlify(hexed) == allbytes)

# Uppercase + lowercase nibble mix across a long buffer.
mixed = ("DEADBEEFcafef00d" * 4).encode()
print("a2b_mixed", binascii.b2a_hex(binascii.a2b_hex(mixed)).decode())
