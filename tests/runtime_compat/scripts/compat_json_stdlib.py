import json

data = {"name": "Molt", "version": 1, "tags": ["wasm", "python"]}
encoded = json.dumps(data, sort_keys=True)
decoded = json.loads(encoded)
print("json roundtrip:", encoded)
print("equal:", data == decoded)
