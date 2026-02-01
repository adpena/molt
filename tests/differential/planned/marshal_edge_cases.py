"""Purpose: differential coverage for marshal edge cases."""

import marshal

payload = (1, 2.5, "hi", b"data", [1, 2], {"x": 3})
blob = marshal.dumps(payload)
print(marshal.loads(blob))
