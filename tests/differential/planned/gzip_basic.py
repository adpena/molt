"""Purpose: differential coverage for gzip basic."""

import gzip
import io


data = b"hello" * 10
buf = io.BytesIO()
with gzip.GzipFile(fileobj=buf, mode="wb") as handle:
    handle.write(data)

payload = buf.getvalue()
roundtrip = gzip.decompress(payload)

print(len(payload) > 0, roundtrip == data)
