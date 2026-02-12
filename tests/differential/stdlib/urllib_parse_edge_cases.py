"""Purpose: differential coverage for urllib.parse edge cases."""

import urllib.parse

print(urllib.parse.quote_plus("a+b c"))
print(urllib.parse.parse_qs("a=1&a=2"))
