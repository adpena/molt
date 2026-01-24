"""Purpose: differential coverage for urllib parse basic."""

import urllib.parse


url = "https://user:pass@example.com:8443/path/../a?x=1&y=2&y=3#frag"
parts = urllib.parse.urlparse(url)
print(parts.scheme, parts.netloc, parts.path, parts.query, parts.fragment)
print(urllib.parse.urlunparse(parts))
print(urllib.parse.urljoin("https://example.com/a/b/", "../c"))
print(urllib.parse.urlencode({"a": 1, "b": "space here"}))
print(urllib.parse.quote("a b/c", safe="/"))
print(urllib.parse.unquote("a%20b%2Fc"))
print(urllib.parse.parse_qs("x=1&y=2&y=3"))
