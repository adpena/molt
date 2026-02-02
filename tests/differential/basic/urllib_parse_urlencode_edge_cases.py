"""Purpose: differential coverage for urllib.parse urlencode edge cases."""

import urllib.parse

print(urllib.parse.urlencode({"a": ["1", "2"]}, doseq=True))
print(urllib.parse.urlencode({"a": "b+c"}))
