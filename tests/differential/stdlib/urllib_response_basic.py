"""Purpose: urllib.response classes expose CPython-style wrapper behavior."""

import io
import urllib.response


events = []


def _close_hook(*args):
    events.append(args)


hooked = urllib.response.addclosehook(io.BytesIO(b"abc"), _close_hook, "token", 7)
print("hook_before", len(events))
hooked.close()
print("hook_after", events)

info_obj = urllib.response.addinfo(io.BytesIO(b"payload"), {"X-Test": "1"})
print("info_value", info_obj.info()["X-Test"])
info_obj.close()

resp = urllib.response.addinfourl(
    io.BytesIO(b"line1\nline2\n"),
    {"Content-Type": "text/plain"},
    "https://example.test/data",
    201,
)
print("url", resp.geturl())
print("code", resp.getcode())
print("status", resp.status)
print("headers", sorted(resp.info().items()))
print("line1", resp.readline().decode("ascii").rstrip())
print("rest", resp.read().decode("ascii").rstrip())
resp.close()
print("closed", resp.fp.closed)
