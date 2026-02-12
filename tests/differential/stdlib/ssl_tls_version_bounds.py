# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for ssl tls version bounds."""

import ssl


ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
ctx.minimum_version = ssl.TLSVersion.TLSv1_2
ctx.maximum_version = ssl.TLSVersion.TLSv1_3

print(ctx.minimum_version == ssl.TLSVersion.TLSv1_2)
print(ctx.maximum_version == ssl.TLSVersion.TLSv1_3)
