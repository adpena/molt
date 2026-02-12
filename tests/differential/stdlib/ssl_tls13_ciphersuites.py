"""Purpose: differential coverage for TLS 1.3 ciphersuites."""

import ssl

ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)

if hasattr(ctx, "set_ciphersuites"):
    ctx.set_ciphersuites("TLS_AES_128_GCM_SHA256")
    names = [cipher["name"] for cipher in ctx.get_ciphers()]
    print("TLS_AES_128_GCM_SHA256" in names)
else:
    print("no_ciphersuites")
