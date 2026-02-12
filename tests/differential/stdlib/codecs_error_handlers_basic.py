"""Purpose: differential coverage for codecs error handler basics."""

import codecs

print(codecs.decode(b"\xff", "utf-8", "replace").encode("unicode_escape").decode("ascii"))
print(codecs.decode(b"\xff", "utf-8", "ignore"))
try:
    codecs.decode(b"\xff", "utf-8", "strict")
except UnicodeDecodeError as exc:
    print(type(exc).__name__)
