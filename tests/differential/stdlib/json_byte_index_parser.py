"""Purpose: differential coverage for byte-indexed intrinsic JSON parsing."""

import json

decoded = json.loads('{"snow":"☃","escaped":"\\u2603","n":123,"f":4.5}')
print(decoded["snow"], decoded["escaped"], decoded["n"], decoded["f"])

for payload in ["[1, 2,]", '{"a": 1,}', '{"a": 1 "b": 2}']:
    try:
        json.loads(payload)
    except json.JSONDecodeError as exc:
        print(exc.lineno, exc.colno, exc.pos)
