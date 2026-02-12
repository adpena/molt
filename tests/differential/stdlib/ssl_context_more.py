# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for ssl context more."""

import ssl


ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE
ctx.set_ciphers("DEFAULT")

print(ctx.verify_mode == ssl.CERT_NONE)
print(ctx.check_hostname)
print(hasattr(ctx, "minimum_version"))
print(hasattr(ctx, "maximum_version"))
