"""Purpose: differential coverage for io.StringIO newline handling."""

import io

buf = io.StringIO()
buf.write("a\n")
buf.write("b\n")
buf.seek(0)
print(buf.readline().strip())
print(buf.readline().strip())
