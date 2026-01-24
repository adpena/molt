# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for ssl context basic."""

import ssl


ctx = ssl.create_default_context()
print("hostname", ctx.check_hostname)
print("verify", ctx.verify_mode == ssl.CERT_REQUIRED)
print("ciphers", len(ctx.get_ciphers()) > 0)
