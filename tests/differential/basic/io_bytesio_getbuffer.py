"""Purpose: differential coverage for io.BytesIO getbuffer."""

import io

buf = io.BytesIO(b"abc")
view = buf.getbuffer()
view[0] = ord("z")
print(buf.getvalue())
