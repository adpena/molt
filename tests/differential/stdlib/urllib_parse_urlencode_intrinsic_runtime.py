"""Purpose: differential coverage for intrinsic-backed urllib.parse.urlencode."""

import urllib.parse

print(urllib.parse.urlencode([("a", "b c"), ("x", "+")]))
print(urllib.parse.urlencode([("a", ["1", "2"]), ("b", ())], doseq=True))
print(urllib.parse.urlencode({"safe": "a/b"}, safe="/"))

try:
    urllib.parse.urlencode([("k",), ("x", "y")])
except Exception as exc:
    print(type(exc).__name__, str(exc))
