"""Purpose: differential coverage for marshal basic API surface."""

import marshal

payload = {"a": 1, "b": [1, 2, 3]}
blob = marshal.dumps(payload)
print(isinstance(blob, (bytes, bytearray)))
print(marshal.loads(blob))
