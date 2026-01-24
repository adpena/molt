"""Purpose: differential coverage for unicode surrogate handling."""

import codecs


s = "\ud800"
try:
    s.encode("utf-8")
except Exception as exc:
    print(type(exc).__name__)

print(s.encode("utf-8", "surrogatepass"))
print(codecs.decode(b"\xed\xa0\x80", "utf-8", "surrogatepass"))
print(s.encode("utf-8", "backslashreplace"))
