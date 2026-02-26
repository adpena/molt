"""Purpose: differential coverage for JSON bytes encoding detection."""

import codecs
import json

payload = '{"snow":"\\u2603","value":1}'
wire = codecs.BOM_UTF16_LE + payload.encode("utf-16-le")
loaded = json.loads(wire)
print(loaded["snow"], loaded["value"])
