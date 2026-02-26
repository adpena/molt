"""Purpose: differential coverage for json.loads bytes/bytearray encoding paths."""

import codecs
import json

payload = '{"snow":"\\u2603","value":1}'
utf16_wire = codecs.BOM_UTF16_LE + payload.encode("utf-16-le")
utf32_wire = codecs.BOM_UTF32_BE + payload.encode("utf-32-be")

loaded16 = json.loads(utf16_wire)
loaded32 = json.loads(bytearray(utf32_wire))
print(loaded16["snow"], loaded16["value"])
print(loaded32["snow"], loaded32["value"])


def parse_constant(token: str):
    return f"const:{token}"


nan_payload = "[NaN, Infinity, -Infinity]"
nan_wire = codecs.BOM_UTF16_BE + nan_payload.encode("utf-16-be")
print(json.loads(nan_wire, parse_constant=parse_constant))
