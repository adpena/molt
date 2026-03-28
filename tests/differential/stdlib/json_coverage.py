import json
d = {"name": "Alice", "age": 30, "scores": [95, 87, 92]}
s = json.dumps(d, sort_keys=True)
print(s)
parsed = json.loads(s)
print(parsed["name"])
print(json.dumps({"a": 1}, indent=2))
for val in [None, True, False, 42, 3.14, "hello", [1, 2], {"k": "v"}]:
    assert json.loads(json.dumps(val)) == val
print("roundtrip OK")
