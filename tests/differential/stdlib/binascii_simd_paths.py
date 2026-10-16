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


class ProtocolBuffer:
    def __init__(self, data):
        self.data = bytearray(data)
        self.events = []
        self.view = None

    def __buffer__(self, flags):
        self.events.append(("get", flags))
        self.view = memoryview(self.data)
        return self.view

    def __release_buffer__(self, view):
        self.events.append(("release", view is self.view))
        self.view = None

import base64
import quopri

for name, operation in (
    ("hex", binascii.hexlify),
    ("crc", binascii.crc32),
    ("b64", base64.b64encode),
    ("b32", base64.b32encode),
    ("b85", base64.b85encode),
    ("quopri", quopri.encodestring),
):
    source = ProtocolBuffer(b"abc=def")
    print("buffer-leaf", name, operation(source), source.events)
    source.data.extend(b"!")
    print("buffer-leaf-unpinned", name, len(source.data))

source = ProtocolBuffer(b"616263")
print("buffer-hex-decode", binascii.unhexlify(source), source.events)
source = ProtocolBuffer(b"YWJj")
print("buffer-base64-decode", base64.b64decode(source), source.events)

class MutationIndex:
    def __index__(self):
        source.events.append(("index",))
        try:
            source.data.extend(b"!")
        except BufferError:
            source.events.append(("pinned",))
        source.data[0] = 65
        return 0

source = ProtocolBuffer(b"abc")
print("buffer-crc-order", binascii.crc32(source, MutationIndex()), source.events)
