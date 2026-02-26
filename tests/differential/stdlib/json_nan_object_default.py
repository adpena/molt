"""Purpose: differential coverage for default NaN handling in object payloads."""

import json

payload = '{"x": NaN, "y": Infinity, "z": -Infinity}'
data = json.loads(payload)
print(type(data["x"]).__name__, type(data["y"]).__name__, type(data["z"]).__name__)
print(json.dumps(data, sort_keys=True))
