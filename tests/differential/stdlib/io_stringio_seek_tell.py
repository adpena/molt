"""Purpose: differential coverage for io.StringIO seek/tell."""

import io

buf = io.StringIO()
buf.write("hello")
print(buf.tell())
buf.seek(0)
print(buf.read(2))
