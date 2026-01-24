"""Purpose: differential coverage for SSL MemoryBIO handshake."""

import ssl

ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE

inbio = ssl.MemoryBIO()
outbio = ssl.MemoryBIO()
obj = ctx.wrap_bio(inbio, outbio, server_side=False)

try:
    obj.do_handshake()
except Exception as exc:
    print(type(exc).__name__)
