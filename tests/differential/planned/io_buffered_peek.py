"""Purpose: ensure BufferedReader.peek behaves as expected."""

import io

raw = io.BytesIO(b"hello")
reader = io.BufferedReader(raw)
print(reader.peek(2)[:2])
print(reader.read(2))
