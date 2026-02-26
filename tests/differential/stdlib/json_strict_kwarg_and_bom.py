"""Purpose: differential coverage for json strict kw forwarding and BOM errors."""

import json


class StrictPresenceDecoder(json.JSONDecoder):
    def __init__(self, **kwargs):
        print("strict_present", "strict" in kwargs, kwargs.get("strict"))
        super().__init__(**kwargs)


print(json.loads('{"a": 1}', cls=StrictPresenceDecoder)["a"])
print(json.loads('{"a": 2}', cls=StrictPresenceDecoder, strict=True)["a"])

payload = '{"x":"a\x00b"}'
try:
    json.loads(payload, cls=StrictPresenceDecoder)
except Exception as exc:  # noqa: BLE001
    print("strict_default_error", type(exc).__name__)
print(json.loads(payload, cls=StrictPresenceDecoder, strict=False)["x"])

try:
    json.loads('\ufeff{"a": 1}')
except Exception as exc:  # noqa: BLE001
    print(type(exc).__name__, getattr(exc, "msg", None), getattr(exc, "pos", None))
