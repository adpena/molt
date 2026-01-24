"""Purpose: differential coverage for SSLContext check_hostname rules."""

import ssl

ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE
print(ctx.check_hostname, ctx.verify_mode == ssl.CERT_NONE)

try:
    ctx.check_hostname = True
except Exception as exc:
    print(type(exc).__name__)
