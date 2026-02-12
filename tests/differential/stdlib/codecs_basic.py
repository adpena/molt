"""Purpose: differential coverage for codecs basic API surface."""

import codecs

print(codecs.lookup("utf-8").name)
print(codecs.encode("hi", "utf-8"))
print(codecs.decode(b"hi", "utf-8"))

replaced = codecs.decode(b"\xff", "utf-8", "replace")
print(replaced.encode("unicode_escape").decode("ascii"))
