"""Purpose: differential coverage for urllib.parse quote/unquote."""

import urllib.parse

q = urllib.parse.quote("a b")
print(q)
print(urllib.parse.unquote(q))
print(urllib.parse.urlencode({"a": "b c"}))
