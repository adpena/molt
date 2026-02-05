"""Purpose: differential coverage for io.BytesIO basics."""

import io

buf = io.BytesIO()
buf.write(b"hi")
print(buf.getvalue())
buf.seek(0)
print(buf.read())
