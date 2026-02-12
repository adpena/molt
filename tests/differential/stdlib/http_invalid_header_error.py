"""Purpose: differential coverage for invalid HTTP headers."""

import http.client

conn = http.client.HTTPConnection("127.0.0.1", timeout=1.0)
try:
    conn.putrequest("GET", "/")
    conn.putheader("X-Test", "bad
value")
    conn.endheaders()
except Exception as exc:
    print(type(exc).__name__)
finally:
    conn.close()
