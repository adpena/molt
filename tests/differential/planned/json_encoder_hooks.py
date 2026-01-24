"""Purpose: differential coverage for json encoder/decoder hooks and options."""

import json


def default(obj):
    return {"__obj__": str(obj)}


payload = {"a": 1, "b": [2, 3], "c": "\u2603"}
text = json.dumps(
    payload,
    default=default,
    ensure_ascii=False,
    sort_keys=True,
    separators=(",", ":"),
    indent=2,
)
print("text", text)

restored = json.loads(
    text,
    object_hook=lambda d: {"hook": True, **d},
    parse_int=lambda s: int(s) + 1,
)
print("restored", restored["a"], restored["hook"])
