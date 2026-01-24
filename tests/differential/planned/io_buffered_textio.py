"""Purpose: differential coverage for io buffered textio."""

import io


raw = io.BytesIO()
buf = io.BufferedWriter(raw)
buf.write(b"hello")
buf.flush()

raw.seek(0)
reader = io.BufferedReader(raw)
print(reader.read().decode())

raw2 = io.BytesIO(b"a\r\n\n")
text = io.TextIOWrapper(raw2, newline=None, encoding="utf-8")
print(text.read())
