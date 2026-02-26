"""Purpose: verify json forwards strict/**kw to decoder/encoder classes."""

import json


payload = "{\"x\":\"a\x00b\"}"
try:
    json.loads(payload)
except Exception as exc:  # noqa: BLE001
    print(type(exc).__name__)

print(json.loads(payload, strict=False)["x"])


class VerboseDecoder(json.JSONDecoder):
    def __init__(self, **kwargs):
        print("decoder kwargs", sorted(kwargs))
        super().__init__(**kwargs)


class VerboseEncoder(json.JSONEncoder):
    def __init__(self, **kwargs):
        print("encoder kwargs", sorted(kwargs))
        super().__init__(**kwargs)


class NoKwDecoder(json.JSONDecoder):
    def __init__(self):
        print("no_kw_decoder")
        super().__init__()


class StrictOnlyDecoder(json.JSONDecoder):
    def __init__(self, strict=True):
        print("strict_only_decoder", strict)
        super().__init__(strict=strict)


class CustomKwDecoder(json.JSONDecoder):
    def __init__(self, marker=None, **kwargs):
        print("custom_marker", marker, sorted(kwargs))
        super().__init__(**kwargs)


print(json.loads('{"a": 1}', cls=VerboseDecoder, strict=False)["a"])
print(json.dumps({"a": 1}, cls=VerboseEncoder, allow_nan=False))
print(json.loads('{"a": 2}', cls=NoKwDecoder)["a"])
print(json.loads('{"a": 3}', cls=StrictOnlyDecoder)["a"])
print(json.loads('{"a": 4}', cls=StrictOnlyDecoder, strict=False)["a"])
print(json.loads('{"a": 5}', cls=CustomKwDecoder, marker="molt")["a"])

try:
    json.loads('{"a": 6}', cls=NoKwDecoder, strict=False)
except Exception as exc:  # noqa: BLE001
    print(type(exc).__name__)
